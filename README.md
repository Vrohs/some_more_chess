# OMACHESS

A chess study application for Omarchy. The point is measurable improvement, not
a chess GUI: puzzles are served at rising difficulty, returned on a spaced
repetition schedule, and **how long you take to solve them** is tracked as the
primary marker of progress.

Phase 1 is the measurement loop, end to end. It runs entirely offline.

## How progress is measured

The application has two modes, and they are kept strictly apart.

**Learn** (the default) serves puzzles you have never seen, rising in difficulty
as your rating rises. This builds your repertoire. **It produces no progress
figure at all**, because a puzzle you have never solved has nothing to be
compared against.

**Repeat** serves back puzzles you have already solved, shuffled, with spaced
repetition deciding which are ripe. These attempts are measured.

The measurement is **paired**: each puzzle's most recent correct solve is
compared against *its own* first correct solve. This matters. Comparing your
early solve times with your later ones across different puzzles measures the
puzzles as much as it measures you — a run of easy forks looks like progress and
a run of hard endgames looks like decline. Pairing removes that confound
entirely.

What is reported, and nothing more:

- the **median speedup** across repeated puzzles, with the number of puzzles behind it
- how many got **faster, slower, and unchanged**
- a one-sided **sign test** p-value: the probability of that many puzzles
  improving by chance alone

Nothing is reported until at least five puzzles have been repeated. Only correct
solves are timed, so a fast wrong answer cannot look like fluency. Ties are
excluded from the test rather than counted as evidence either way. A result that
is not distinguishable from chance is labelled as such rather than presented as
improvement.

Spaced repetition (FSRS) decides *when* a solved puzzle comes back. Your solve
time relative to your own pace decides the grade it is scheduled with: well
inside your usual pace grades Easy and waits longer, laboured grades Hard and
returns sooner, wrong grades Again.

Difficulty comes from the puzzle's own published rating, tracked by a plain Elo
update against your personal rating. The floor is 1100 — nothing easier is ever
stored or served.

## Getting the puzzles

The Lichess puzzle export is CC0:

```sh
curl -O https://database.lichess.org/lichess_db_puzzle.csv.zst
omachess ingest lichess_db_puzzle.csv.zst
```

Puzzles below 1100 are discarded during ingest. Ingesting again is idempotent.

## Running

```sh
omachess                 # open the trainer
omachess status          # what is stored, what is due
omachess progress        # time-to-solve trend per rating band
omachess ingest FILE     # load the puzzle export
```

The board is shown from the solving side, so a puzzle where Black moves is
played with Black at the bottom.

There are deliberately **no legal-move hints**. Highlighting where a piece may
go turns a tactic into a short list to check, which makes puzzles faster in a
way that has nothing to do with getting better — and solve time is the number
this application exists to measure. The answer is revealed after two wrong
moves, by which point the attempt already counts as failed.

## Running the build you just made

The desktop entry runs `omachess` from `PATH`. If a *copy* is sitting there it
will go stale the moment you rebuild, and the application will behave nothing
like the source in front of you — this has happened, and it cost a day of
confusion. Link it instead:

    ./scripts/install-local.sh

`omachess --version` reports which binary is running and when it was built, so
the question is answerable rather than mysterious.

## Development

The toolchain is pinned with `mise`; nothing is installed system-wide.

```sh
mise install
cargo test --workspace
./scripts/dev.sh          # run against isolated state
./scripts/dev.sh status
```

`scripts/dev.sh` redirects every XDG directory into `./.devhome`, so a
development build cannot read or write your real review database. Reset it with
`rm -rf .devhome`. Your GTK configuration is linked in rather than isolated, so
a development run still looks like the installed application.

The domain core (`crates/omachess-core`) has no GUI and no network dependency,
which is where the scheduling and grading tests live.

Engine tests are skipped unless an engine is pointed at explicitly, so the suite
runs anywhere:

```sh
OMACHESS_TEST_ENGINE=/usr/bin/stockfish cargo test --workspace
```

## Theming

The window, the progress charts and all the chrome style themselves with
libadwaita's named colours, which Omarchy generates from the active theme. They
follow your theme with no configuration.

**The board itself does not.** Squares are wood, and pieces are black and white,
because those are properties of a chess board rather than of a desktop palette —
a scarlet or lilac board reads as a toy, and a "black" piece that is actually
mid-grey is hard to tell from a white one at a glance. Black pieces are
multiplied down toward true black at load time for the same reason.

If you would rather the board follow your theme, install the template:

```sh
cp themes/omachess.css.tpl ~/.config/omarchy/themed/
```

Omarchy renders it into the active theme directory on the next theme switch, and
OMACHESS loads it over its built-in stylesheet, replacing the wood.

Stylesheets are read at startup, so a theme switch needs a restart.

## Piece sets

Piece art is **not** shipped with this repository. Sets are installed per user:

```
~/.local/share/omachess/pieces/<set-name>/{w,b}{K,Q,R,B,N,P}.svg
```

The naming matches Lichess, so any Lichess piece set works by copying its
directory there. OMACHESS picks up the set named by `OMACHESS_PIECE_SET`, or the
first one installed, and falls back to Unicode glyphs when none is present.

Nothing is vendored because piece art carries its own licence, and those licences
vary widely — several popular Lichess sets are CC BY-NC-SA, which is not
compatible with redistribution here. Check the licence of any set before
redistributing it. Lichess documents them in
[lila's COPYING.md](https://github.com/lichess-org/lila/blob/master/COPYING.md);
note that some sets in that repository, **including `governor`, are not listed
there at all** and therefore have no licence you can rely on.

SVGs are rasterised with `resvg` rather than through GTK, because current
librsvg no longer installs a gdk-pixbuf loader.

## Playing the engine

The Play view puts you against Stockfish, capped near your own rating rather
than at full strength — being crushed by a 3600-rated engine teaches nothing.
Install it first:

```sh
yay -S stockfish
```

There is no evaluation bar and no takeback while you play. Both would tell you
that you had gone wrong before you had the chance to notice, which is the one
skill a game trains that a puzzle cannot.

When the game ends it is reviewed at full strength, and every position you
misjudged can be added to your deck with one click. Those puzzles then flow
through the same spaced repetition and the same solve-time measurement as
downloaded ones — a loss on Tuesday becomes a drill on Friday.

Mistakes are graded by **win probability**, not material: giving up a pawn near
equality matters far more than giving one up while already winning.

### What the panel tells you while you play

The opening is named as you play it — "Ruy Lopez: Morphy Defense (C70)" — along
with the move you left the book at. That is knowledge about what you are
playing, not a hint about the position in front of you, which is why it is safe
to show mid-game where an evaluation is not. The data is the Lichess opening
book (CC0; see `assets/openings/COPYING.txt`).

Alongside it: material balance, and the scoresheet annotated with how long you
spent on each of your own moves.

### Did you blunder, or did you rush?

Every game report splits your moves at the median think time and compares how
much each half gave away, with a one-sided Mann-Whitney test. If your errors are
genuinely concentrated in the moves you rushed, the report says so — and the fix
is a habit rather than more tactics, which is a different piece of advice
entirely. If speed and error are not reliably linked, it says that instead.

This is the one question the engine cannot answer alone: it needs how long each
move took, which only the application knows.

### Tracking how well you play, not how often you win

Whether you won says as much about how hard the opponent was set as about how
you played. So each finished game is recorded by its **quality**: mean accuracy,
win probability given away per move, error counts, and which phase of the game
leaked most.

`omachess games` lists recent games and compares the median accuracy of the
later half against the earlier half with a one-sided **Mann-Whitney U test**.
Nothing is compared until ten games exist, because the test uses a normal
approximation that means little on smaller samples. This is a weaker design than
the puzzle measurement — games are not paired, and no two are alike — but the
opponent is pinned near your own rating, which keeps them broadly comparable.

## Getting the puzzles, faster

`database.lichess.org` throttles hard — measured under 10 kB/s from some
networks, which makes the 290 MB export a twelve-hour download even on a fast
connection. `scripts/fetch-puzzles.sh` fetches it in parallel ranges instead and
resumes safely if interrupted:

```sh
./scripts/fetch-puzzles.sh
```

## Not yet built

Opening study, game import, and the networked Lichess lookups (opening explorer,
tablebase, cloud evaluation) are all deferred. The puzzle
trainer is deliberately local: solve times are measured in seconds, and a
network round trip inside the solve loop would corrupt the measurement.

## Packaging

`packaging/` holds a `PKGBUILD`, a desktop entry and an icon. The package
depends only on `gtk4` and `libadwaita`; SQLite is compiled in, and SVG
rasterisation is pure Rust, so neither is a runtime dependency. `check()` runs
the domain-core tests, which need no display.

Piece sets are not packaged — see above.

## Licence

MIT. See `LICENSE`.
