# Sourced by safety.sh: volumes, the CLI wrapper, checks and the verdict table.

TREE="python3 $SAFETY/tree.py"
RESULTS=()

say() { printf '%s\n' "$*" >&2; }

# record NAME pass|fail EVIDENCE
record() {
  RESULTS+=("$1|$2|$3")
  printf '%-5s %-18s %s\n' "$(tr '[:lower:]' '[:upper:]' <<<"$2")" "$1" "$3" >&2
}

# check NAME FAILURES EVIDENCE: passes when FAILURES is 0
check() {
  if [[ $2 -eq 0 ]]; then record "$1" pass "$3"; else record "$1" fail "$3"; fi
}

# image NAME FS MEGABYTES [readonly]: a sparse image mounted below $WORK/mnt, never in /Volumes
image() {
  local name=$1 fs=$2 size=$3 mode=${4:-}
  local file=$WORK/images/$name.sparseimage mount=$WORK/mnt/$name
  mkdir -p "$WORK/images" "$mount"
  if [[ ! -f $file ]]; then
    hdiutil create -quiet -size "${size}m" -type SPARSE -fs "$fs" -volname "S$name" "$file" 2>/dev/null ||
      { say "could not create a $fs image"; return 1; }
  fi
  local flags=(-nobrowse -noverify -noautoopen -mountpoint "$mount")
  [[ $mode == readonly ]] && flags+=(-readonly)
  hdiutil attach -quiet "${flags[@]}" "$file" 2>/dev/null || { say "could not attach $file"; return 1; }
  printf '%s\n' "$mount"
}

mounted() { mount | grep -qF " on $1 ("; }

detach() {
  mounted "$1" && { hdiutil detach -quiet "$1" 2>/dev/null || hdiutil detach -quiet -force "$1" 2>/dev/null; }
  rmdir "$1" 2>/dev/null
  return 0
}

# Every image this run mounted, found in the mount table: `image` runs in a subshell.
detach_all() {
  local mount
  mount | awk -v under="$WORK/mnt/" 'index($3, under) == 1 {print $3}' | while IFS= read -r mount; do
    detach "$mount"
  done
  rm -rf "${WORK:?}/images"
}

# Locks, deny entries and 000 modes would stop the clean-up.
unlock() {
  chflags -R nouchg,noschg "$@" 2>/dev/null
  chmod -R -N "$@" 2>/dev/null
  chmod -R u+rwX "$@" 2>/dev/null
  return 0
}

# fresh NAME: an empty scratch folder on the boot volume
fresh() {
  [[ -e $WORK/$1 ]] && { unlock "$WORK/$1"; rm -rf "${WORK:?}/$1"; }
  mkdir -p "$WORK/$1"
  printf '%s\n' "$WORK/$1"
}

# founder ARGS...: the CLI with JSON output into $WORK/last.jsonl; returns its exit status
founder() {
  "$FOUNDER" --json "$@" >"$WORK/last.jsonl" 2>"$WORK/last.err"
}

# launch ARGS...: the CLI in the background, output in $WORK/bg.jsonl; sets PID
launch() {
  : >"$WORK/bg.jsonl"
  "$FOUNDER" --json "$@" >"$WORK/bg.jsonl" 2>"$WORK/bg.err" &
  PID=$!
}

# bounded SECONDS PID: waits for PID, killing it after SECONDS; returns its status or 124
bounded() {
  local secs=$1 pid=$2 ticks=0
  while kill -0 "$pid" 2>/dev/null; do
    sleep 0.1
    ticks=$((ticks + 1))
    if ((ticks > secs * 10)); then
      kill -9 "$pid" 2>/dev/null
      wait "$pid" 2>/dev/null
      return 124
    fi
  done
  wait "$pid"
}

# wait_for EVENT: until the background CLI prints that event, or it ends
wait_for() {
  local tries=0
  until grep -q "\"event\":\"$1\"" "$WORK/bg.jsonl" 2>/dev/null; do
    kill -0 "$PID" 2>/dev/null || return 1
    sleep 0.02
    tries=$((tries + 1))
    ((tries > 3000)) && return 1
  done
  return 0
}

# field NAME [FILE]: a field of the last JSON line in FILE (default the last CLI run)
field() {
  python3 - "${2:-$WORK/last.jsonl}" "$1" <<'PY'
import json, sys
lines = [line for line in open(sys.argv[1]) if line.strip().startswith("{")]
event = json.loads(lines[-1]) if lines else {}
value = event.get(sys.argv[2])
print(json.dumps(value, ensure_ascii=False) if isinstance(value, (list, dict)) else ("" if value is None else value))
PY
}

# errors [FILE]: every error message the CLI printed
errors() {
  python3 - "${1:-$WORK/last.jsonl}" <<'PY'
import json, sys
for line in open(sys.argv[1]):
    if line.strip().startswith("{"):
        event = json.loads(line)
        for failure in event.get("failures") or []:
            print(failure["error"])
        if event.get("error"):
            print(event["error"])
PY
}

# progress FIELD [FILE]: that field of the last progress line
progress() {
  python3 - "${2:-$WORK/last.jsonl}" "$1" <<'PY'
import json, sys
seen = [json.loads(line) for line in open(sys.argv[1]) if '"event":"progress"' in line]
print(seen[-1].get(sys.argv[2], "") if seen else "")
PY
}

manifest() { $TREE manifest "$@"; }
temps() { $TREE temps "$@"; }
count_temps() { temps "$1" | wc -l | tr -d ' '; }
sha() { shasum -a 256 "$1" | cut -d' ' -f1; }

# same TREE MANIFEST [FIELDS]: the tree matches the manifest; prints the differences
same() {
  manifest "$1" >"$WORK/now.json" || return 1
  $TREE compare "$2" "$WORK/now.json" ${3:+--fields "$3"} --skip-temps
}

# has TEXT [FILE]: some error the CLI printed contains TEXT
has() { errors "${2:-}" | grep -qF -- "$1"; }

table() {
  local row name verdict evidence passed=0 failed=0 skipped=0
  printf '\n| property | verdict | evidence |\n|---|---|---|\n'
  for row in "${RESULTS[@]}"; do
    IFS='|' read -r name verdict evidence <<<"$row"
    printf '| %s | %s | %s |\n' "$name" "$verdict" "${evidence//|//}"
    case $verdict in pass) passed=$((passed + 1)) ;; skip) skipped=$((skipped + 1)) ;; *) failed=$((failed + 1)) ;; esac
  done
  printf '\n%d passed, %d failed, %d skipped\n' "$passed" "$failed" "$skipped"
  ((failed == 0))
}
