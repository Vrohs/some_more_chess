#!/usr/bin/env bash
# Download the Lichess puzzle export into the corpus directory.
#
# database.lichess.org throttles a single connection hard — measured at under
# 10 kB/s from some networks, which is a twelve-hour download. It throttles the
# whole transfer rather than each connection, but several connections still
# come out several times faster, so the file is fetched in parallel ranges and
# reassembled.
#
# Every part resumes, so interrupting and re-running this is safe.
set -uo pipefail

URL="https://database.lichess.org/lichess_db_puzzle.csv.zst"
# Lichess answers 429 beyond a handful of concurrent range requests, so this
# stays low deliberately: more connections finish slower, not faster.
PARTS="${PARTS:-6}"
DEST_DIR="${XDG_DATA_HOME:-$HOME/.local/share}/omachess/corpus"
DEST="$DEST_DIR/$(basename "$URL")"
WORK="$DEST_DIR/.parts"

mkdir -p "$WORK"

total=$(curl -sI --max-time 60 "$URL" | awk 'tolower($1)=="content-length:"{print $2+0}' | tail -1)
if [[ -z "$total" || "$total" -le 0 ]]; then
    echo "could not determine the download size" >&2
    exit 1
fi

if [[ -f "$DEST" ]] && [[ "$(stat -c%s "$DEST")" -eq "$total" ]]; then
    echo "already complete: $DEST"
    exit 0
fi

echo "fetching $((total / 1000000)) MB in $PARTS parallel parts"
chunk=$(( (total + PARTS - 1) / PARTS ))

fetch_part() {
    local index=$1 start=$2 end=$3
    local part="$WORK/part.$index"
    local want=$(( end - start + 1 ))
    local backoff=1

    for _ in $(seq 1 400); do
        local have=0
        [[ -f "$part" ]] && have=$(stat -c%s "$part")

        # More bytes than were asked for means an interrupted transfer was
        # appended twice. The duplication is somewhere in the middle, so the
        # part cannot be repaired and is fetched again from scratch.
        if (( have > want )); then
            rm -f "$part"
            have=0
        fi
        (( have == want )) && return 0

        # --fail matters more than it looks: without it a 429 rate-limit reply
        # writes its error page into the part as though it were file content.
        #
        # And deliberately no --retry: curl's own retry restarts the range from
        # its beginning while the shell keeps appending, duplicating bytes
        # silently. Each invocation makes one attempt; the loop resumes from
        # whatever actually reached the disk.
        if curl -sS --fail --max-time 900 --speed-limit 512 --speed-time 60 \
                -r "$(( start + have ))-$end" "$URL" >> "$part" 2>>"$WORK/errors.log"
        then
            backoff=1
        else
            # Almost always a 429. Backing off is what clears it.
            sleep "$backoff"
            backoff=$(( backoff < 30 ? backoff * 2 : 30 ))
        fi
        sleep 1
    done
    return 1
}

for (( i = 0; i < PARTS; i++ )); do
    start=$(( i * chunk ))
    end=$(( start + chunk - 1 ))
    (( end >= total )) && end=$(( total - 1 ))
    (( start > end )) && continue
    fetch_part "$i" "$start" "$end" &
done
wait

# Reassemble only once every part is the size it should be.
for (( i = 0; i < PARTS; i++ )); do
    start=$(( i * chunk ))
    end=$(( start + chunk - 1 ))
    (( end >= total )) && end=$(( total - 1 ))
    (( start > end )) && continue
    want=$(( end - start + 1 ))
    have=$(stat -c%s "$WORK/part.$i" 2>/dev/null || echo 0)
    if [[ "$have" -ne "$want" ]]; then
        echo "part $i is $have of $want bytes; re-run to continue" >&2
        exit 1
    fi
done

cat "$WORK"/part.* > "$DEST"
got=$(stat -c%s "$DEST")
if [[ "$got" -ne "$total" ]]; then
    echo "assembled $got bytes, expected $total" >&2
    exit 1
fi

# Size alone does not prove the bytes are in the right order, so the archive is
# decompressed and checked before anything is thrown away.
echo "verifying..."
if ! zstd -t "$DEST" 2>/dev/null; then
    echo "the assembled file is not a valid zstd archive; discarding parts" >&2
    rm -f "$DEST"
    rm -rf "$WORK"
    exit 1
fi

rm -rf "$WORK"
echo "complete: $DEST ($((got / 1000000)) MB)"
echo "now run: omachess ingest \"$DEST\""
