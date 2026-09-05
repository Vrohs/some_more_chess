//! Talking to a UCI chess engine.
//!
//! The engine runs as a sandboxed child process. It is fed positions and
//! returns evaluations; it never touches the database, the network, or the
//! filesystem beyond what it needs to start.

use anyhow::{anyhow, bail, Context, Result};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

/// Centipawns-to-win-probability constant, as used by Lichess.
const WIN_CHANCE_SCALE: f64 = 0.003_682_08;

/// How hard the engine should think about one position.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    Depth(u32),
    Nodes(u64),
    Movetime(Duration),
    /// Search to a depth but give up after a time, whichever comes first.
    ///
    /// Depth alone is unbounded in practice: most positions reach a given depth
    /// quickly and a few take many times longer, which is enough to make an
    /// interface feel stuck.
    DepthOrTime {
        depth: u32,
        millis: u64,
    },
}

impl Limit {
    fn to_go_args(self) -> String {
        match self {
            Limit::Depth(d) => format!("depth {d}"),
            Limit::Nodes(n) => format!("nodes {n}"),
            Limit::Movetime(t) => format!("movetime {}", t.as_millis()),
            Limit::DepthOrTime { depth, millis } => format!("depth {depth} movetime {millis}"),
        }
    }
}

/// An evaluation from the point of view of the side to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Score {
    Cp(i32),
    /// Moves until mate; negative means the side to move is being mated.
    Mate(i32),
}

impl Score {
    /// Probability that the side to move wins, in `0.0..=1.0`.
    ///
    /// Centipawns are not linear in practical terms — the difference between
    /// +1 and +2 matters far more than between +8 and +9 — so mistakes are
    /// judged on this scale rather than on raw material.
    pub fn win_chance(self) -> f64 {
        match self {
            Score::Cp(cp) => 1.0 / (1.0 + (-WIN_CHANCE_SCALE * f64::from(cp)).exp()),
            Score::Mate(n) if n >= 0 => 1.0,
            Score::Mate(_) => 0.0,
        }
    }
}

/// What the engine concluded about a position.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub best_move: Option<String>,
    pub score: Option<Score>,
    pub depth: u32,
    /// Principal variation, in UCI notation.
    pub pv: Vec<String>,
}

pub struct Engine {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    id: String,
}

impl Engine {
    /// Start an engine directly, without a sandbox. Prefer [`Engine::spawn`].
    pub fn spawn_unsandboxed(program: &Path) -> Result<Self> {
        let mut command = Command::new(program);
        Self::start(
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null()),
        )
        .with_context(|| format!("starting engine {}", program.display()))
    }

    /// Start an engine inside a `bwrap` sandbox when one is available.
    ///
    /// An engine is a large C++ binary fed positions from a downloaded corpus.
    /// It has no reason to reach the network or write anything, so it is denied
    /// both; a crash or a hostile input then costs nothing.
    pub fn spawn(program: &Path) -> Result<Self> {
        let Some(bwrap) = which("bwrap") else {
            return Self::spawn_unsandboxed(program);
        };
        let mut command = Command::new(bwrap);
        command
            .args(["--unshare-net", "--unshare-pid", "--die-with-parent"])
            .args(["--ro-bind", "/", "/"])
            .args(["--dev", "/dev"])
            .args(["--proc", "/proc"])
            .args(["--tmpfs", "/tmp"]);

        // The tmpfs above hides anything under /tmp, including the engine
        // itself if it happens to live there, and any network weights beside
        // it. Binding its directory back afterwards — bwrap applies mounts in
        // order — keeps the sandbox while leaving the engine reachable.
        if let Some(dir) = program.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            command.arg("--ro-bind").arg(dir).arg(dir);
        }

        command
            .arg("--")
            .arg(program)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        Self::start(&mut command)
            .with_context(|| format!("starting {} in a bwrap sandbox", program.display()))
    }

    fn start(command: &mut Command) -> Result<Self> {
        let mut child = command.spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("no engine stdin"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("no engine stdout"))?;
        let mut engine = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            id: String::new(),
        };
        engine.handshake()?;
        Ok(engine)
    }

    fn handshake(&mut self) -> Result<()> {
        self.send("uci")?;
        let mut id = String::new();
        loop {
            let line = self.read_line()?;
            if let Some(name) = line.strip_prefix("id name ") {
                id = name.trim().to_owned();
            }
            if line.trim() == "uciok" {
                break;
            }
        }
        self.id = id;
        self.ready()
    }

    /// The engine's self-reported name, such as `Stockfish 18`.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn ready(&mut self) -> Result<()> {
        self.send("isready")?;
        loop {
            if self.read_line()?.trim() == "readyok" {
                return Ok(());
            }
        }
    }

    pub fn set_option(&mut self, name: &str, value: &str) -> Result<()> {
        self.send(&format!("setoption name {name} value {value}"))?;
        self.ready()
    }

    /// Cap the engine's playing strength, or remove the cap with `None`.
    ///
    /// Stockfish will not go below roughly 1320, so weaker requests are raised
    /// to its floor rather than silently ignored.
    pub fn limit_strength(&mut self, elo: Option<u32>) -> Result<()> {
        match elo {
            Some(elo) => {
                self.set_option("UCI_LimitStrength", "true")?;
                self.set_option("UCI_Elo", &elo.max(MIN_LIMITED_ELO).to_string())
            }
            None => self.set_option("UCI_LimitStrength", "false"),
        }
    }

    /// Evaluate a position, optionally after playing `moves` from it.
    /// Analyse a position asking for the best `lines` moves rather than one.
    ///
    /// A position only makes a fair puzzle when its best move is the *only*
    /// good move. With one line the engine will happily name a move while an
    /// equally good alternative sits beside it, and the solver who finds the
    /// alternative is told they were wrong. Comparing the top two is the only
    /// way to know.
    pub fn analyse_lines(
        &mut self,
        fen: &str,
        moves: &[String],
        limit: Limit,
        lines: u32,
    ) -> Result<Vec<Analysis>> {
        let lines = lines.max(1);
        self.set_option("MultiPV", &lines.to_string())?;
        let position = if moves.is_empty() {
            format!("position fen {fen}")
        } else {
            format!("position fen {fen} moves {}", moves.join(" "))
        };
        self.send(&position)?;
        self.send(&format!("go {}", limit.to_go_args()))?;

        // Every line is re-sent at each depth, so the last report for each
        // index is the deepest and the one worth keeping.
        let mut found: Vec<Analysis> = Vec::new();
        loop {
            let line = self.read_line()?;
            let line = line.trim();
            if line.starts_with("bestmove ") {
                break;
            }
            if !line.starts_with("info ") {
                continue;
            }
            let Some(index) = multipv_index(line) else {
                continue;
            };
            let mut entry = Analysis::default();
            merge_info(&mut entry, line);
            // The move is the head of the variation: `bestmove` only ever
            // names the first line.
            entry.best_move = entry.pv.first().cloned();
            if entry.best_move.is_none() {
                continue;
            }
            let slot = index.saturating_sub(1) as usize;
            if found.len() <= slot {
                found.resize_with(slot + 1, Analysis::default);
            }
            found[slot] = entry;
        }
        // Restore the default so ordinary analysis is unaffected.
        self.set_option("MultiPV", "1")?;
        found.retain(|entry| entry.best_move.is_some());
        Ok(found)
    }

    pub fn analyse(&mut self, fen: &str, moves: &[String], limit: Limit) -> Result<Analysis> {
        let position = if moves.is_empty() {
            format!("position fen {fen}")
        } else {
            format!("position fen {fen} moves {}", moves.join(" "))
        };
        self.send(&position)?;
        self.send(&format!("go {}", limit.to_go_args()))?;

        let mut analysis = Analysis::default();
        loop {
            let line = self.read_line()?;
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("bestmove ") {
                let best = rest.split_whitespace().next().unwrap_or_default();
                analysis.best_move =
                    (best != "(none)" && !best.is_empty()).then(|| best.to_owned());
                return Ok(analysis);
            }
            if line.starts_with("info ") {
                merge_info(&mut analysis, line);
            }
        }
    }

    fn send(&mut self, line: &str) -> Result<()> {
        writeln!(self.stdin, "{line}").context("writing to engine")?;
        self.stdin.flush().context("flushing to engine")
    }

    fn read_line(&mut self) -> Result<String> {
        let mut line = String::new();
        let read = self
            .stdout
            .read_line(&mut line)
            .context("reading from engine")?;
        if read == 0 {
            bail!(
                "the engine produced no output and exited; if it is sandboxed, \
                 check that its binary and any network files are readable inside \
                 the sandbox"
            );
        }
        Ok(line)
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "quit");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

/// Stockfish refuses to be weakened below this.
pub const MIN_LIMITED_ELO: u32 = 1320;

/// Parse one `info` line into the running analysis.
/// The `multipv` index on an info line, if it carries one.
fn multipv_index(line: &str) -> Option<u32> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let at = tokens.iter().position(|t| *t == "multipv")?;
    tokens.get(at + 1)?.parse().ok()
}

fn merge_info(analysis: &mut Analysis, line: &str) {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "depth" => {
                if let Some(value) = tokens.get(index + 1).and_then(|d| d.parse().ok()) {
                    analysis.depth = value;
                }
                index += 2;
            }
            "score" => {
                match (tokens.get(index + 1), tokens.get(index + 2)) {
                    (Some(&"cp"), Some(value)) => {
                        if let Ok(cp) = value.parse() {
                            analysis.score = Some(Score::Cp(cp));
                        }
                    }
                    (Some(&"mate"), Some(value)) => {
                        if let Ok(n) = value.parse() {
                            analysis.score = Some(Score::Mate(n));
                        }
                    }
                    _ => {}
                }
                index += 3;
            }
            "pv" => {
                analysis.pv = tokens[index + 1..]
                    .iter()
                    .map(|m| (*m).to_owned())
                    .collect();
                break;
            }
            _ => index += 1,
        }
    }
}

/// Locate an executable on `PATH`.
pub fn which(program: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(program);
            candidate.is_file().then_some(candidate)
        })
    })
}

/// The strongest engine available on this machine, if any.
pub fn find_engine() -> Option<PathBuf> {
    ["stockfish", "lc0"].into_iter().find_map(which)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_chance_is_even_at_zero_and_saturates() {
        assert!((Score::Cp(0).win_chance() - 0.5).abs() < 1e-9);
        assert!(Score::Cp(1000).win_chance() > 0.97);
        assert!(Score::Cp(-1000).win_chance() < 0.03);
        assert_eq!(Score::Mate(3).win_chance(), 1.0);
        assert_eq!(Score::Mate(-3).win_chance(), 0.0);
    }

    #[test]
    fn a_pawn_matters_far_more_when_the_game_is_level() {
        let level = Score::Cp(100).win_chance() - Score::Cp(0).win_chance();
        let winning = Score::Cp(900).win_chance() - Score::Cp(800).win_chance();
        assert!(
            level > winning * 3.0,
            "expected diminishing returns, got {level} vs {winning}"
        );
    }

    #[test]
    fn info_lines_are_merged_into_one_analysis() {
        let mut analysis = Analysis::default();
        merge_info(
            &mut analysis,
            "info depth 18 seldepth 24 multipv 1 score cp -37 nodes 1000 pv e2e4 e7e5 g1f3",
        );
        assert_eq!(analysis.depth, 18);
        assert_eq!(analysis.score, Some(Score::Cp(-37)));
        assert_eq!(analysis.pv, vec!["e2e4", "e7e5", "g1f3"]);
    }

    #[test]
    fn mate_scores_are_parsed() {
        let mut analysis = Analysis::default();
        merge_info(&mut analysis, "info depth 5 score mate 2 pv d1h5");
        assert_eq!(analysis.score, Some(Score::Mate(2)));
    }

    #[test]
    fn later_info_lines_win() {
        let mut analysis = Analysis::default();
        merge_info(&mut analysis, "info depth 4 score cp 10 pv a2a3");
        merge_info(&mut analysis, "info depth 20 score cp 55 pv e2e4");
        assert_eq!(analysis.depth, 20);
        assert_eq!(analysis.score, Some(Score::Cp(55)));
        assert_eq!(analysis.pv, vec!["e2e4"]);
    }

    #[test]
    fn limit_arguments_match_the_uci_vocabulary() {
        assert_eq!(Limit::Depth(12).to_go_args(), "depth 12");
        assert_eq!(Limit::Nodes(50_000).to_go_args(), "nodes 50000");
        assert_eq!(
            Limit::Movetime(Duration::from_millis(250)).to_go_args(),
            "movetime 250"
        );
        assert_eq!(
            Limit::DepthOrTime {
                depth: 22,
                millis: 3000
            }
            .to_go_args(),
            "depth 22 movetime 3000",
            "both bounds are sent so whichever comes first wins"
        );
    }
}
