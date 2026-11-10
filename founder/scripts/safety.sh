#!/usr/bin/env bash
# usage: safety.sh [--work DIR] [--founder PATH] [--keep] [test...]
#   Proves the file engine's safety properties through the `founder-cli` binary: kill -9 mid-move and
#   `founder-cli clean` after it, cancels, full, read-only and vanishing volumes, locks, permissions,
#   links, odd names, FAT, conflicts, undo, concurrency and big trees. Fixtures live on sparse disk
#   images mounted under DIR (default a temp folder), never in /Volumes, and go when the run ends.
set -uo pipefail
SAFETY=$(cd "$(dirname "$0")/safety" && pwd)
ROOT=$(cd "$SAFETY/../../.." && pwd)

WORK='' FOUNDER='' KEEP=0 ONLY=()
while (($#)); do
  case $1 in
    --work) WORK=$2; shift 2 ;;
    --founder) FOUNDER=$2; shift 2 ;;
    --keep) KEEP=1; shift ;;
    -*) sed -n '2,6s/^# \{0,1\}//p' "$0" >&2; exit 64 ;;
    *) ONLY+=("$1"); shift ;;
  esac
done
WORK=${WORK:-$(mktemp -d "${TMPDIR:-/tmp}/founder-safety.XXXXXX")}
mkdir -p "$WORK" && WORK=$(cd "$WORK" && pwd)
if [[ -z $FOUNDER ]]; then
  (cd "$ROOT" && cargo build --release -p founder-cli >&2) || exit 1
  FOUNDER=${CARGO_TARGET_DIR:-$ROOT/target}/release/founder-cli
fi

source "$SAFETY/lib.sh"

finish() {
  detach_all
  if ((KEEP == 0)); then
    unlock "$WORK"
    rm -rf "$WORK"
  fi
}
trap finish EXIT
trap 'exit 130' INT TERM HUP

NOTES=() BAD=0
begin() { NOTES=(); BAD=0; }
note() { NOTES+=("$*"); }
# want DESCRIPTION COMMAND...: counts a failure, and says so, when COMMAND fails
want() {
  local what=$1
  shift
  "$@" || { BAD=$((BAD + 1)); NOTES+=("NOT $what"); }
}
done_test() {
  local IFS=';'
  check "$1" "$BAD" "${NOTES[*]}"
}
is() { [[ $1 == "$2" ]]; }
absent() { [[ ! -e $1 && ! -L $1 ]]; }
none() { [[ -z $1 || $1 == 0 ]]; }

# Sources may vanish only once the destination holds all of them: a poller watches a move.
t_poll_move() {
  begin
  local src dst
  src=$(fresh poll) && dst=$(image poll APFS 1024) || return
  $TREE tree "$src/Tree" --count 1500 --max 60000 --seed 11 --big big.bin:150000000
  manifest "$src/Tree" >"$WORK/poll.json"
  rm -f "$WORK/poll.stop"
  $TREE watch "$src/Tree" "$dst/Tree" "$WORK/poll.json" "$WORK/poll.stop" "$WORK/poll.report" &
  local watcher=$!
  sleep 0.3
  founder move "$src/Tree" "$dst"
  local code=$?
  touch "$WORK/poll.stop"
  wait "$watcher"
  local watched=$?
  note "exit $code" "poller $(cat "$WORK/poll.report")"
  want "exit 0" is "$code" 0
  want "every source left only after its copy was whole" is "$watched" 0
  want "the moved tree matches" same "$dst/Tree" "$WORK/poll.json" >/dev/null
  want "the source folder is gone" absent "$src/Tree"
  done_test poll_move
}

# kill_rounds KIND ROUNDS: kill -9 a move or copy at random moments, check, then merge to finish.
kill_rounds() {
  local kind=$1 rounds=$2 name=kill_$1
  begin
  local dst src round code split merged left mid=0 lost=0 litter=0 again=0 dirty=0
  dst=$(image "$name" APFS 2048) || return
  for ((round = 1; round <= rounds; round++)); do
    src=$(fresh "$name-src")
    $TREE tree "$src/Tree" --count 2500 --max 60000 --seed $((round + 20)) \
      --big a.bin:80000000 --big b.bin:60000000
    manifest "$src/Tree" >"$WORK/$name.json"
    rm -rf "$dst/Tree" "$dst/.Trashes/$(id -u)"
    launch "$kind" "$src/Tree" "$dst"
    sleep "0.$((RANDOM % 10))$((RANDOM % 10))"
    kill -9 "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null
    grep -q '"event":"result"' "$WORK/bg.jsonl" || mid=$((mid + 1))
    $TREE split "$src/Tree" "$dst/Tree" "$WORK/$name.json" >"$WORK/$name.split"
    split=$?
    if ((split != 0)); then
      lost=$((lost + 1))
      note "round $round: $(cat "$WORK/$name.split")"
    fi
    litter=$((litter + $(count_temps "$dst")))
    if [[ -e $src/Tree ]]; then
      founder "$kind" --conflict merge "$src/Tree" "$dst"
      code=$?
      if ((code != 0)); then
        again=$((again + 1))
        note "round $round: merge exit $code: $(errors | head -1)"
      fi
    fi
    same "$dst/Tree" "$WORK/$name.json" >"$WORK/$name.diff"
    merged=$?
    if ((merged != 0)); then
      again=$((again + 1))
      note "round $round: after the merge $(head -2 "$WORK/$name.diff" | tr '\n' ' ')"
    fi
    if [[ $kind == move && -e $src/Tree ]]; then
      again=$((again + 1))
      note "round $round: the merged move left its source"
    fi
    left=$(count_temps "$dst")
    if ((left != 0)); then
      dirty=$((dirty + 1))
      note "round $round: $left temps after the merge: $(temps "$dst" | head -2 | tr '\n' ' ')"
    fi
  done
  note "$rounds rounds, $mid killed mid-run" "last split $(cat "$WORK/$name.split")" \
    "temp files the killed runs left: $litter"
  want "every file whole at its source or its destination after each kill" is "$lost" 0
  want "a merge re-run finishing every round" is "$again" 0
  want "no temp left after each merge" is "$dirty" 0
  done_test "$name"
}
t_kill_move() { kill_rounds move 8; }
t_kill_copy() { kill_rounds copy 6; }

# A tree past PATH_MAX goes through the descriptor-relative path; kill it mid-file too.
t_kill_deep() {
  begin
  local dst src round code left mid=0 lost=0 litter=0 again=0 dirty=0 seen=''
  dst=$(image killdeep APFS 1024) || return
  for ((round = 1; round <= 8; round++)); do
    src=$(fresh killdeep-src)
    mkdir "$src/Deep"
    $TREE deep "$src/Deep" --length 1300 --file-size 30000000 >/dev/null
    manifest "$src/Deep" >"$WORK/killdeep.json"
    rm -rf "$dst/Deep" "$dst/.Trashes/$(id -u)"
    launch move "$src/Deep" "$dst"
    sleep "0.$((RANDOM % 6))$((RANDOM % 10))"
    kill -9 "$PID" 2>/dev/null
    wait "$PID" 2>/dev/null
    grep -q '"event":"result"' "$WORK/bg.jsonl" || mid=$((mid + 1))
    if ! $TREE split "$src/Deep" "$dst/Deep" "$WORK/killdeep.json" >"$WORK/killdeep.split"; then
      lost=$((lost + 1))
      note "round $round: $(cat "$WORK/killdeep.split")"
    fi
    seen+="$(field temps "$WORK/killdeep.split" | python3 -c 'import json, sys; print(len(json.load(sys.stdin)))') "
    litter=$((litter + $(count_temps "$dst")))
    if [[ -e $src/Deep ]]; then
      founder move --conflict merge "$src/Deep" "$dst"
      code=$?
      ((code == 0)) || { again=$((again + 1)); note "round $round: merge exit $code: $(errors | head -1)"; }
    fi
    same "$dst/Deep" "$WORK/killdeep.json" >/dev/null || { again=$((again + 1)); note "round $round: tree differs"; }
    [[ -e $src/Deep ]] && { again=$((again + 1)); note "round $round: the merged move left its source"; }
    left=$(count_temps "$dst")
    ((left == 0)) || { dirty=$((dirty + 1)); note "round $round: $left temps after the merge"; }
  done
  note "8 rounds, $mid killed mid-run" "temps the split saw per round: $seen" \
    "temp files the killed runs left: $litter"
  want "nothing partial under a real name past PATH_MAX" is "$lost" 0
  want "a merge re-run finishing every round, its source gone" is "$again" 0
  want "no temp left after each merge" is "$dirty" 0
  done_test kill_deep
}

# clean deletes stale temps, trashes a stale Replace's item, keeps live temps and other names.
t_kill_cleanup() {
  begin
  local src dst trash dead apid bpid rdir code unique f ahead bhead atrash='' btrash='' mine decoys before_mine before_decoys
  src=$(fresh cleanup) && dst=$(image cleanup APFS 1024) || return
  trash=$dst/.Trashes/$(id -u)
  sleep 0 &
  dead=$!
  wait "$dead"
  $TREE tree "$src/A" --count 4 --max 20000 --seed 61 --big big.bin:150000000
  $TREE tree "$src/B" --count 0 --seed 62 --big big.bin:150000000
  $TREE tree "$src/E" --count 4 --max 20000 --seed 63 --big big.bin:150000000
  manifest "$src/E" >"$WORK/cleanup.json"
  ahead=$(head -c 1048576 "$src/A/big.bin" | shasum -a 256)
  bhead=$(head -c 1048576 "$src/B/big.bin" | shasum -a 256)
  mkdir -p "$dst/Docs/inner" "$dst/Work" "$dst/.finder-fs-123-4"
  head -c 5000000 /dev/urandom >"$dst/big.bin"
  echo kept >"$dst/keep.txt" && echo inner >"$dst/Docs/inner/note.txt"
  echo old-format >"$dst/.finder-fs-123-4/inside.txt"
  mine=(big.bin keep.txt Docs/inner/note.txt)
  decoys=(.finder-fs-123-4/inside.txt .finder-fs-staging-12-x-3 .finder-fs-staging-12-3 .finder-fs-notes.txt
    finder-fs-staging-1-2-3 ".finder-fs-replace-$dead-1-0.txt" Docs/.finder-fs-99-1)
  for f in "${decoys[@]:1}"; do echo "decoy $f" >"$dst/$f"; done
  before_mine=$(cd "$dst" && shasum -a 256 "${mine[@]}" 2>&1)
  before_decoys=$(cd "$dst" && shasum -a 256 "${decoys[@]}" 2>&1)
  launch copy --pause-after 20M "$src/A" "$dst"
  want "the copy paused mid-file before its kill" wait_for paused
  apid=$PID
  kill -9 "$PID" 2>/dev/null
  wait "$PID" 2>/dev/null
  launch copy --conflict replace --pause-after 20M "$src/B/big.bin" "$dst"
  want "the Replace paused mid-file before its kill" wait_for paused
  bpid=$PID
  kill -9 "$PID" 2>/dev/null
  wait "$PID" 2>/dev/null
  unique="swapped out by job $dead, run $$ $RANDOM $(date +%s)"
  mkdir "$dst/Docs/.finder-fs-replace-$dead-1-0" && echo "$unique" >"$dst/Docs/.finder-fs-replace-$dead-1-0/old.txt"
  echo reused >"$dst/.finder-fs-staging-$$-1-7"
  launch copy --pause-after 20M "$src/E" "$dst/Work"
  want "the live copy paused mid-file" wait_for paused
  temps --exact "$dst" >"$WORK/cleanup.before"
  grep -E "^Work/.*\.finder-fs-(staging|replace)-$PID-" "$WORK/cleanup.before" >"$WORK/cleanup.live"
  rdir=$(grep -m1 "^\.finder-fs-replace-$bpid-[0-9]*-[0-9]*$" "$WORK/cleanup.before")
  note "before clean: $(tr '\n' ' ' <"$WORK/cleanup.before")"
  want "the killed copy left a staging temp" grep -q "^A/\.finder-fs-staging-$apid-[0-9]*-[0-9]*$" "$WORK/cleanup.before"
  want "the killed Replace left its folder holding big.bin" test -n "$rdir" -a -f "$dst/$rdir/big.bin"
  want "the live copy has temps" test -s "$WORK/cleanup.live"
  founder clean "$dst"
  code=$?
  temps --exact "$dst" >"$WORK/cleanup.after"
  while IFS= read -r f; do
    case $(head -c 1048576 "$f" | shasum -a 256) in
      "$ahead") atrash=$f ;;
      "$bhead") btrash=$f ;;
    esac
  done < <(find "$trash" -type f 2>/dev/null)
  note "clean: exit $code, left $(tr '\n' ' ' <"$WORK/cleanup.after")" \
    "Trash: $(cd "$trash" 2>/dev/null && ls -A | tr '\n' ' ')" \
    "the Replace's big.bin in the Trash: $(basename "${btrash:-none}") $([[ -n $btrash ]] && stat -f %z "$btrash") bytes"
  want "clean exits 0" is "$code" 0
  want "no stale temp left" is "$(grep -vE "^Work/.*\.finder-fs-(staging|replace)-$PID-" "$WORK/cleanup.after")" ""
  want "the Replace's partial big.bin in the Trash" test -n "$btrash"
  want "the swapped-out old.txt in the Trash" grep -rqxF "$unique" "$trash"
  want "the stale staging copy deleted, not trashed" test -z "$atrash"
  want "the destination's own files unchanged" is "$(cd "$dst" && shasum -a 256 "${mine[@]}" 2>&1)" "$before_mine"
  want "every name outside the temp format unchanged" is "$(cd "$dst" && shasum -a 256 "${decoys[@]}" 2>&1)" "$before_decoys"
  want "the live copy's temps kept" is "$(grep -E "^Work/.*\.finder-fs-(staging|replace)-$PID-" "$WORK/cleanup.after")" \
    "$(cat "$WORK/cleanup.live")"
  kill -USR1 "$PID"
  bounded 120 "$PID"
  code=$?
  note "the live copy resumed: exit $code, $(count_temps "$dst/Work") temps"
  want "the live copy exits 0" is "$code" 0
  want "its tree whole" same "$dst/Work/E" "$WORK/cleanup.json" >/dev/null
  want "no temp left where it landed" none "$(count_temps "$dst/Work")"
  mkdir -p "$dst/G/.finder-fs-replace-$dead-1-8"
  echo stale >"$dst/G/.finder-fs-staging-$dead-1-9"
  echo "$unique again" >"$dst/G/.finder-fs-replace-$dead-1-8/swapped.txt"
  echo decoy >"$dst/G/.finder-fs-$dead-9"
  echo small >"$src/small.txt"
  founder copy "$src/small.txt" "$dst/G"
  code=$?
  note "copy into a folder holding stale temps: exit $code, left $(ls -A "$dst/G" | tr '\n' ' ')"
  want "the copy exits 0" is "$code" 0
  want "the copy landed" is "$(cat "$dst/G/small.txt" 2>/dev/null)" small
  want "the copy cleaned the stale temps where it landed" is "$(temps --exact "$dst/G")" ""
  want "the stale Replace's item trashed by the copy" grep -rqxF "$unique again" "$trash"
  want "an old-format name kept by the copy" is "$(cat "$dst/G/.finder-fs-$dead-9" 2>/dev/null)" decoy
  done_test kill_cleanup
}

# A cancel ends cleanly: no temp file, nothing partial, every file in one place.
t_cancel() {
  begin
  local src dst code
  src=$(fresh cancel) && dst=$(image cancel APFS 1024) || return
  $TREE tree "$src/Tree" --count 400 --max 100000 --seed 5 --big big.bin:200000000
  manifest "$src/Tree" >"$WORK/cancel.json"
  founder copy --cancel-after 40M "$src/Tree" "$dst"
  code=$?
  note "copy cancelled after 40 MB: exit $code, $(count_temps "$dst") temps"
  want "a cancelled copy exits 3" is "$code" 3
  want "no temp file after a cancelled copy" none "$(count_temps "$dst")"
  want "the copy's sources untouched" same "$src/Tree" "$WORK/cancel.json"
  want "nothing partial after the cancelled copy" $TREE split "$src/Tree" "$dst/Tree" "$WORK/cancel.json"
  rm -rf "$dst/Tree"
  launch move --pause-after 30M "$src/Tree" "$dst"
  wait_for paused
  kill -INT "$PID"
  bounded 60 "$PID"
  code=$?
  $TREE split "$src/Tree" "$dst/Tree" "$WORK/cancel.json" >"$WORK/cancel.split"
  local split=$?
  note "move cancelled by Control-C while paused: exit $code, $(count_temps "$dst") temps, $(cat "$WORK/cancel.split")"
  want "Control-C exits 3" is "$code" 3
  want "no temp file after Control-C" none "$(count_temps "$dst")"
  want "every file whole in one place after Control-C" is "$split" 0
  done_test cancel
}

t_full_upfront() {
  begin
  local src dst code
  src=$(fresh fullup) && dst=$(image fullup HFS+ 24) || return
  mkdir "$src/Tree"
  for i in 1 2 3; do head -c 10000000 /dev/urandom >"$src/Tree/f$i.bin"; done
  manifest "$dst" >"$WORK/fullup.json"
  founder copy "$src/Tree" "$dst"
  code=$?
  note "30 MB into a 24 MB volume: exit $code: $(errors | head -1)"
  want "exit 2" is "$code" 2
  want "the volume named as too full" has "enough space"
  want "nothing written" same "$dst" "$WORK/fullup.json" >/dev/null
  founder move "$src/Tree/f1.bin" "$src/Tree/f2.bin" "$src/Tree/f3.bin" "$dst"
  code=$?
  note "three files moved: exit $code"
  want "a refused move exits 2" is "$code" 2
  want "the refused move wrote nothing" same "$dst" "$WORK/fullup.json" >/dev/null
  want "the refused move kept its sources" test -f "$src/Tree/f3.bin"
  done_test full_upfront
}

# More files than workers, so some start after the volume fills: a file in flight holds its room.
t_full_midway() {
  begin
  local src dst code
  src=$(fresh fullmid) && dst=$(image fullmid HFS+ 64) || return
  mkdir "$src/Tree"
  for i in $(seq 10 25); do head -c 3000000 /dev/urandom >"$src/Tree/f$i.bin"; done
  manifest "$src/Tree" >"$WORK/fullmid.json"
  launch move --pause-after 3M "$src/Tree" "$dst"
  wait_for paused
  dd if=/dev/zero of="$dst/filler" bs=1048576 2>/dev/null
  kill -USR1 "$PID"
  bounded 120 "$PID"
  code=$?
  note "filled while paused: exit $code: $(errors "$WORK/bg.jsonl" | head -1)"
  want "a stop, not success" test "$code" -eq 1 -o "$code" -eq 2
  want "the full volume named" has "enough space" "$WORK/bg.jsonl"
  want "no temp file" none "$(count_temps "$dst")"
  $TREE split "$src/Tree" "$dst/Tree" "$WORK/fullmid.json" >"$WORK/fullmid.split"
  want "every file whole in one place" test $? -eq 0
  note "split $(cat "$WORK/fullmid.split")"
  rm -f "$dst/filler"
  founder move --conflict merge "$src/Tree" "$dst"
  code=$?
  note "merge after freeing room: exit $code"
  want "the merge finishes" is "$code" 0
  want "the tree whole after the merge" same "$dst/Tree" "$WORK/fullmid.json" >/dev/null
  done_test full_midway
}

t_read_only() {
  begin
  local src ro dst code
  src=$(fresh ro) && ro=$(image ro HFS+ 64) && dst=$(image rodst APFS 256) || return
  $TREE tree "$ro/Tree" --count 40 --max 50000 --seed 8
  detach "$ro"
  ro=$(image ro HFS+ 64 readonly) || return
  manifest "$ro" >"$WORK/ro.json"
  echo new >"$src/new.txt"
  founder copy "$src/new.txt" "$ro"
  code=$?
  note "copy into it: exit $code: $(errors | head -1)"
  want "copying in exits 2" is "$code" 2
  want "the volume named read only" has "read only"
  founder move "$ro/Tree" "$dst"
  code=$?
  note "move out of it: exit $code: $(errors | head -1)"
  want "moving out exits 1" is "$code" 1
  want "the originals named as kept" has "can't be removed"
  want "the read-only volume unchanged" same "$ro" "$WORK/ro.json" >/dev/null
  manifest "$ro/Tree" >"$WORK/ro.tree.json"
  want "every file copied out" same "$dst/Tree" "$WORK/ro.tree.json" >/dev/null
  done_test read_only
}

t_locked() {
  begin
  local src vol code
  src=$(fresh lock) && vol=$(image lock APFS 256) || return
  echo one >"$src/locked.txt" && chflags uchg "$src/locked.txt"
  founder copy "$src/locked.txt" "$vol"
  code=$?
  note "copy exit $code, copy flags $(stat -f %Sf "$vol/locked.txt" 2>/dev/null)"
  want "a copy keeps the lock" grep -q uchg <<<"$(stat -f %Sf "$vol/locked.txt" 2>/dev/null)"
  mkdir "$vol/moved"
  founder move "$src/locked.txt" "$vol/moved"
  code=$?
  note "move across volumes: exit $code: $(errors | head -1)"
  want "a move of a locked file exits 1" is "$code" 1
  want "its source kept" test -f "$src/locked.txt"
  want "its copy landed" test -f "$vol/moved/locked.txt"
  founder trash "$vol/locked.txt"
  code=$?
  want "a trash without --unlock refused" is "$code" 2
  want "and nothing moved" test -f "$vol/locked.txt"
  founder trash --unlock "$vol/locked.txt"
  code=$?
  want "trash --unlock works" is "$code" 0
  mkdir -p "$vol/F" && echo x >"$vol/F/in.txt" && chflags uchg "$vol/F/in.txt"
  founder delete "$vol/F"
  code=$?
  note "delete of a folder holding a locked file: exit $code"
  want "the folder deleted" absent "$vol/F"
  echo y >"$vol/r.txt" && chflags uchg "$vol/r.txt"
  founder rename "$vol/r.txt" s.txt
  code=$?
  note "rename: exit $code: $(errors | head -1)"
  want "a locked item not renamed" test -f "$vol/r.txt"
  want "rename refused" is "$code" 2
  done_test locked
}

t_unreadable() {
  begin
  local src dst code
  src=$(fresh unread) && dst=$(image unread APFS 256) || return
  mkdir "$src/F" && echo ok >"$src/F/ok.txt" && echo secret >"$src/F/secret.txt"
  chmod 000 "$src/F/secret.txt"
  founder copy "$src/F" "$dst"
  code=$?
  note "copy: exit $code: $(errors | head -1)"
  want "copy exits 1" is "$code" 1
  want "the readable file copied" test -f "$dst/F/ok.txt"
  want "no partial secret" absent "$dst/F/secret.txt"
  want "no temp" none "$(count_temps "$dst")"
  rm -rf "$dst/F"
  founder move "$src/F" "$dst"
  code=$?
  note "move: exit $code"
  want "move exits 1" is "$code" 1
  want "the readable file moved" test -f "$dst/F/ok.txt" -a ! -e "$src/F/ok.txt"
  chmod 644 "$src/F/secret.txt"
  want "the unreadable file kept whole" is "$(cat "$src/F/secret.txt" 2>/dev/null)" secret
  done_test unreadable
}

t_acl_denied() {
  begin
  local src dst code
  src=$(fresh acl) && dst=$(image acl APFS 256) || return
  mkdir -p "$src/F/Denied" && echo a >"$src/F/a.txt" && echo d >"$src/F/Denied/d.txt"
  chmod +a "everyone deny list,search,readattr,readextattr,readsecurity" "$src/F/Denied"
  founder copy "$src/F" "$dst"
  code=$?
  note "copy past a deny-list folder: exit $code: $(errors | head -1)"
  want "copy exits 1" is "$code" 1
  want "the rest copied" test -f "$dst/F/a.txt"
  echo k >"$src/F/keep.txt" && chmod +a "everyone deny delete" "$src/F/keep.txt"
  rm -rf "$dst/F"
  founder move "$src/F" "$dst"
  code=$?
  note "move of a delete-denied file: exit $code: $(errors | grep -i keep | head -1)"
  want "move exits 1" is "$code" 1
  want "the delete-denied original kept" is "$(cat "$src/F/keep.txt" 2>/dev/null)" k
  want "its copy landed" is "$(cat "$dst/F/keep.txt" 2>/dev/null)" k
  want "no temp left beside it" none "$(count_temps "$dst")"
  chmod -R -N "$src/F"
  want "the denied folder kept whole" is "$(cat "$src/F/Denied/d.txt" 2>/dev/null)" d
  done_test acl_denied
}

t_into_itself() {
  begin
  local vol code shape
  vol=$(image itself APFS 256) || return
  mkdir -p "$vol/Folder/inner" && echo x >"$vol/Folder/inner/x.txt" && ln -s Folder "$vol/link"
  manifest "$vol" >"$WORK/itself.json"
  for shape in "move:$vol/Folder/inner" "copy:$vol/Folder/inner" "move:$vol/Folder" \
    "copy:$vol/folder/inner" "copy:$vol/link/inner" "move:$vol/FOLDER/inner"; do
    launch "${shape%%:*}" "$vol/Folder" "${shape#*:}"
    bounded 30 "$PID"
    code=$?
    note "${shape%%:*} into ${shape#*:} exit $code"
    want "${shape%%:*} into ${shape#*:} refused" is "$code" 2
  done
  want "the tree untouched" same "$vol" "$WORK/itself.json" >/dev/null
  done_test into_itself
}

t_same_file() {
  begin
  local vol code
  vol=$(image same APFS 256) || return
  mkdir -p "$vol/d1" "$vol/d2" "$vol/h"
  echo one >"$vol/d1/a.txt"
  founder move "$vol/d1/a.txt" "$vol/d1"
  code=$?
  want "a move onto itself changes nothing" is "$code:$(ls "$vol/d1" | tr '\n' ',')" "0:a.txt,"
  founder copy "$vol/d1/a.txt" "$vol/d1"
  want "a copy into its own folder duplicates" test -f "$vol/d1/a copy.txt"
  rm -f "$vol/d1/a copy.txt"
  ln "$vol/d1/a.txt" "$vol/d2/a.txt"
  founder move "$vol/d1/a.txt" "$vol/d2"
  code=$?
  note "move onto its own hard link elsewhere: exit $code, left $(ls "$vol/d1" | tr '\n' ' ')/ there $(ls "$vol/d2" | tr '\n' ' ')"
  want "the moved link left its folder" absent "$vol/d1/a.txt"
  want "its content kept" is "$(cat "$vol/d2/a.txt")" one
  echo x >"$vol/h/x" && ln "$vol/h/x" "$vol/h/y"
  mkdir "$vol/hc"
  founder copy "$vol/h" "$vol/hc"
  want "both hard links copied" is "$(cat "$vol/hc/h/x" "$vol/hc/h/y" 2>/dev/null | tr '\n' ,)" "x,x,"
  echo case >"$vol/d1/c.txt"
  founder rename "$vol/d1/c.txt" C.txt
  code=$?
  want "a case-only rename takes" is "$code:$(ls "$vol/d1" | grep -i '^c.txt$')" "0:C.txt"
  done_test same_file
}

t_symlinks() {
  begin
  local src dst code
  src=$(fresh links) && dst=$(image links APFS 256) || return
  mkdir -p "$src/S/sub" && echo t >"$src/S/a.txt" && echo s >"$src/S/sub/s.txt"
  ln -s a.txt "$src/S/to-file"
  ln -s sub "$src/S/to-dir"
  ln -s nowhere "$src/S/dangling"
  ln -s loop2 "$src/S/loop1" && ln -s loop1 "$src/S/loop2"
  ln -s . "$src/S/self"
  ln -s "$src/S" "$src/S/sub/up"
  manifest "$src/S" >"$WORK/links.json"
  launch copy "$src/S" "$dst"
  bounded 60 "$PID"
  code=$?
  note "copy exit $code"
  want "copy exits 0" is "$code" 0
  want "links copied as links, never followed" same "$dst/S" "$WORK/links.json" >/dev/null
  mkdir "$dst/m"
  launch move "$src/S" "$dst/m"
  bounded 60 "$PID"
  code=$?
  want "move exits 0" is "$code" 0
  want "moved links unchanged" same "$dst/m/S" "$WORK/links.json" >/dev/null
  want "the source gone" absent "$src/S"
  mkdir "$dst/one"
  founder copy "$dst/m/S/to-dir" "$dst/one"
  want "a link to a folder copies as a link" test -L "$dst/one/to-dir"
  done_test symlinks
}

t_case_names() {
  begin
  local cs ci code
  cs=$(image cs "Case-sensitive APFS" 256) && ci=$(image ci APFS 256) || return
  mkdir "$cs/D" && echo lower >"$cs/D/x.txt" && echo upper >"$cs/D/X.txt"
  founder copy "$cs/D" "$ci"
  code=$?
  note "copy of x.txt and X.txt: exit $code, landed $(ls "$ci/D" | tr '\n' ' ')"
  want "both contents copied" is "$(cat "$ci/D"/* | sort | tr '\n' ,)" "lower,upper,"
  mkdir "$ci/m"
  founder move "$cs/D" "$ci/m"
  code=$?
  note "move: exit $code, landed $(ls "$ci/m/D" 2>/dev/null | tr '\n' ' ')"
  want "both contents moved, nothing lost" is "$(cat "$ci/m/D"/* "$cs/D"/* 2>/dev/null | sort | tr '\n' ,)" "lower,upper,"
  echo new >"$cs/A.txt" && echo old >"$ci/a.txt"
  founder copy "$cs/A.txt" "$ci"
  note "A.txt onto a.txt kept both as $(field results)"
  want "the old a.txt kept" is "$(cat "$ci/a.txt")" old
  want "the new one beside it" is "$(cat "$ci/A 2.txt" 2>/dev/null)" new
  done_test case_names
}

t_unicode() {
  begin
  local src vol fs code
  src=$(fresh uni)
  mkdir -p "$src/U/$(printf 'U\xcc\x88ber')" "$src/twin"
  printf nfc >"$src/U/nfc-caf$(printf '\xc3\xa9').txt"
  printf nfd >"$src/U/nfd-cafe$(printf '\xcc\x81').txt"
  printf deep >"$src/U/$(printf 'U\xcc\x88ber')/$(printf '\xe2\x84\xab')ngstr$(printf '\xc3\xb6')m.txt"
  manifest "$src/U" >"$WORK/uni.json"
  for fs in APFS HFS+ ExFAT; do
    vol=$(image "uni${fs//[^A-Za-z]/}" "$fs" 128) || continue
    founder copy "$src/U" "$vol"
    code=$?
    manifest "$vol/U" >"$WORK/uni.$fs.json"
    $TREE compare --nfd "$WORK/uni.json" "$WORK/uni.$fs.json" >"$WORK/uni.diff"
    want "names and contents equal on $fs" test $? -eq 0 -a "$code" -eq 0
    note "$fs: exit $code $(head -1 "$WORK/uni.diff")"
  done
  vol=$WORK/mnt/uniAPFS
  printf other >"$src/twin/nfc-cafe$(printf '\xcc\x81').txt"
  founder copy "$src/twin/nfc-cafe$(printf '\xcc\x81').txt" "$vol/U"
  note "a decomposed name copied onto its composed twin: $(field results)"
  want "a composed twin is a conflict that keeps both" \
    is "$(cat "$vol/U/nfc-caf$(printf '\xc3\xa9').txt"):$($TREE count "$vol/U")" 'nfc:{"entries": 5, "files": 4, "dirs": 1}'
  done_test unicode
}

t_odd_names() {
  begin
  local src vol fat code long
  src=$(fresh names) && vol=$(image names APFS 256) && fat=$(image namesx ExFAT 128) || return
  long=$(printf 'x%.0s' {1..251}).txt
  mkdir "$src/N"
  echo colon >"$src/N/a:b.txt"
  echo newline >"$src/N/$(printf 'new\nline.txt')"
  echo long >"$src/N/$long"
  echo smile >"$src/N/emoji-$(printf '\xf0\x9f\x99\x82').txt"
  manifest "$src/N" >"$WORK/names.json"
  founder copy "$src/N" "$vol"
  code=$?
  want "odd names copy exactly to APFS" same "$vol/N" "$WORK/names.json" >/dev/null
  founder copy "$src/N" "$fat"
  code=$?
  note "to ExFAT: exit $code, landed $($TREE count "$fat/N"): $(errors | head -2 | tr '\n' ' ')"
  want "no temp on ExFAT" none "$(count_temps "$fat")"
  founder copy "$src/N/$long" "$vol/N"
  code=$?
  note "keep both on a 255-byte name: exit $code $(errors | head -1)"
  want "keep both fits a 255-byte name" is "$code" 0
  founder duplicate "$vol/N/$long"
  code=$?
  note "duplicate: exit $code $(errors | head -1)"
  want "duplicate fits a 255-byte name" is "$code" 0
  founder alias "$vol/N/$long"
  code=$?
  note "alias: exit $code $(errors | head -1)"
  want "an alias fits a 255-byte name" is "$code" 0
  done_test odd_names
}

t_path_max() {
  begin
  local src dst code
  src=$(fresh deep) && dst=$(image deep APFS 512) || return
  mkdir "$src/Deep"
  note "$($TREE deep "$src/Deep" --length 1700)"
  manifest "$src/Deep" >"$WORK/deep.json"
  founder copy "$src/Deep" "$dst"
  code=$?
  want "copy exits 0" is "$code" 0
  want "the deep tree copies whole" same "$dst/Deep" "$WORK/deep.json" >/dev/null
  mkdir "$dst/m"
  founder move "$src/Deep" "$dst/m"
  code=$?
  want "move exits 0" is "$code" 0
  want "the deep tree moves whole" same "$dst/m/Deep" "$WORK/deep.json" >/dev/null
  want "its source gone" absent "$src/Deep"
  founder trash "$dst/Deep"
  code=$?
  want "the deep tree trashed" is "$code" 0
  done_test path_max
}

make_meta() {
  local at=$1
  mkdir -p "$at/M"
  echo body >"$at/M/doc.txt"
  echo hidden >"$at/M/hidden.txt" && chflags hidden "$at/M/hidden.txt"
  xattr -wx com.apple.ResourceFork "$(head -c 300 /dev/urandom | xxd -p | tr -d '\n')" "$at/M/doc.txt"
  xattr -w com.example.note kept "$at/M/doc.txt"
  xattr -w com.apple.quarantine "0081;5f000000;Safari;" "$at/M/doc.txt"
  "$FOUNDER" tag add Red "$at/M/doc.txt" >/dev/null
  "$FOUNDER" comment "$at/M/doc.txt" "a comment" >/dev/null
  SetFile -d "01/02/2020 03:04:05" "$at/M/doc.txt"
  SetFile -d "05/06/2019 07:08:09" "$at/M"
  chmod +a "staff allow read" "$at/M/doc.txt"
  chmod 640 "$at/M/doc.txt"
  xattr -w com.example.folder yes "$at/M"
}

t_metadata() {
  begin
  local src vol fs code
  src=$(fresh meta)
  make_meta "$src"
  # HFS+ keeps the hidden flag in FinderInfo, so that file's attributes differ by design.
  manifest --meta "$src/M" --ignore .DS_Store --ignore hidden.txt >"$WORK/meta.json"
  for fs in APFS HFS+; do
    vol=$(image "meta${fs//[^A-Za-z]/}" "$fs" 128) || continue
    founder copy "$src/M" "$vol"
    code=$?
    manifest --meta "$vol/M" --ignore .DS_Store --ignore hidden.txt >"$WORK/meta.$fs.json"
    want "the hidden flag kept on $fs" grep -q hidden <<<"$(stat -f %Sf "$vol/M/hidden.txt")"
    $TREE compare "$WORK/meta.json" "$WORK/meta.$fs.json" \
      --fields sha256,xattrs,acl,flags,mode,birth_s,mtime_s >"$WORK/meta.diff"
    want "fork, xattrs, tags, quarantine, dates, ACL, flags and mode kept on $fs" test $? -eq 0 -a "$code" -eq 0
    note "$fs: $(head -2 "$WORK/meta.diff" | tr '\n' ' ')"
    "$FOUNDER" --json info "$vol/M/doc.txt" >"$WORK/meta.info"
    want "the comment shows on $fs" is "$(field comment "$WORK/meta.info")" "a comment"
    want "the tag shows on $fs" grep -q '"Red"' "$WORK/meta.info"
  done
  done_test metadata
}

t_fat_litter() {
  begin
  local src vol fs code litter stores
  src=$(fresh fat)
  mkdir -p "$src/L/sub"
  echo plain >"$src/L/plain.txt"
  echo plain >"$src/L/sub/plain2.txt"
  echo q >"$src/L/quarantined.txt" && xattr -w com.apple.quarantine "0081;5f000000;Safari;" "$src/L/quarantined.txt"
  echo t >"$src/L/tagged.txt" && "$FOUNDER" tag add Blue "$src/L/tagged.txt" >/dev/null
  echo f >"$src/L/forked.txt" && xattr -wx com.apple.ResourceFork "$(head -c 600 /dev/urandom | xxd -p | tr -d '\n')" "$src/L/forked.txt"
  echo c >"$src/L/commented.txt" && "$FOUNDER" comment "$src/L/commented.txt" "made a store" >/dev/null
  for fs in ExFAT "MS-DOS FAT32"; do
    vol=$(image "fat${fs//[^A-Za-z0-9]/}" "$fs" 128) || continue
    founder copy "$src/L" "$vol"
    code=$?
    litter=$(cd "$vol" && find L -name '._*' | sort | tr '\n' ' ')
    stores=$(cd "$vol" && find . -path ./.Trashes -prune -o -name .DS_Store -print | wc -l | tr -d ' ')
    note "$fs: exit $code; ._ files: ${litter:-none}; .DS_Store: $stores"
    want "copy to $fs exits 0" is "$code" 0
    want "no ._ beside plain files on $fs" test -z "$(grep -E '\._(plain|plain2)\.txt' <<<"$litter")"
    want "the resource fork carried to $fs" grep -q '\._forked\.txt' <<<"$litter"
    want "no .DS_Store on $fs" none "$stores"
    want "tags carried to $fs" grep -q Blue <<<"$("$FOUNDER" tag list "$vol/L/tagged.txt")"
  done
  local ex=$WORK/mnt/fatExFAT fat=$WORK/mnt/fatMSDOSFAT32
  mkdir "$fat/m"
  founder move "$ex/L" "$fat/m"
  code=$?
  note "ExFAT to FAT32 move: exit $code, left $(ls -A "$ex" | grep -v -e '^\.fseventsd$' -e '^\.Trashes$' | tr '\n' ' ')"
  want "a FAT to FAT move takes the ._ files with their files" absent "$ex/L"
  want "no .DS_Store after the move" none "$(cd "$fat" && find . -path ./.Trashes -prune -o -name .DS_Store -print | wc -l | tr -d ' ')"
  done_test fat_litter
}

t_fat32_4gb() {
  begin
  local src vol code before after
  src=$(fresh fat4g) && vol=$(image fat4g "MS-DOS FAT32" 8192) || return
  $TREE sparse "$src/huge.bin" 4500000000
  echo small >"$src/small.txt"
  before=$(du -k "$WORK/images/fat4g.sparseimage" | cut -f1)
  founder copy "$src/huge.bin" "$vol"
  code=$?
  note "4.5 GB sparse file: exit $code: $(errors | head -1)"
  want "refused with exit 2" is "$code" 2
  want "named too large for the format" has "too large"
  founder copy "$src/huge.bin" "$src/small.txt" "$vol"
  code=$?
  want "the small one beside it still lands (exit 1)" is "$code:$(cat "$vol/small.txt" 2>/dev/null)" "1:small"
  after=$(du -k "$WORK/images/fat4g.sparseimage" | cut -f1)
  note "image grew $((after - before)) KB"
  want "nothing large written" test $((after - before)) -lt 10240
  want "no huge file or temp on the volume" test ! -e "$vol/huge.bin" -a "$(count_temps "$vol")" -eq 0
  rm -f "$src/huge.bin"
  done_test fat32_4gb
}

t_conflicts() {
  begin
  local src vol code
  src=$(fresh conf) && vol=$(image conf APFS 256) || return
  echo new >"$src/a.txt" && mkdir "$src/F" && echo newx >"$src/F/x" && echo z >"$src/F/z"
  echo old >"$vol/a.txt" && mkdir "$vol/F" && echo oldx >"$vol/F/x" && echo y >"$vol/F/y"
  founder copy "$src/a.txt" "$src/F" "$vol"
  founder copy "$src/a.txt" "$src/F" "$vol"
  note "keep both twice: $(ls "$vol" | grep -v '^\.' | tr '\n' ' ')"
  want "keep-both names a 2, a 3, F 2, F 3" test -f "$vol/a 2.txt" -a -f "$vol/a 3.txt" -a -d "$vol/F 2" -a -d "$vol/F 3"
  want "the originals untouched" is "$(cat "$vol/a.txt" "$vol/F/x")" "$(printf 'old\noldx')"
  founder copy --conflict skip "$src/a.txt" "$vol"
  code=$?
  want "skip leaves both" is "$code:$(cat "$vol/a.txt"):$(cat "$src/a.txt")" "0:old:new"
  founder copy --conflict replace --journal "$WORK/conf.journal" "$src/a.txt" "$vol"
  want "replace puts the new one in place" is "$(cat "$vol/a.txt")" new
  want "the replaced one went to the Trash" grep -rqx old "$vol/.Trashes/$(id -u)"
  founder undo "$WORK/conf.journal"
  want "undo brings the replaced one back" is "$(cat "$vol/a.txt")" old
  founder copy --conflict merge --journal "$WORK/merge.journal" "$src/F" "$vol"
  want "merge folds the folders, files replace" is "$(cat "$vol/F/x" "$vol/F/y" "$vol/F/z" | tr '\n' ,)" "newx,y,z,"
  founder undo "$WORK/merge.journal"
  want "undo of the merge restores the folder" is "$(ls "$vol/F" | tr '\n' ,):$(cat "$vol/F/x")" "x,y,:oldx"
  done_test conflicts
}

# undo_trip NAME FIELDS COMMAND...: undo restores the tree from before, redo the one after
undo_trip() {
  local name=$1 fields=$2
  shift 2
  local journal=$WORK/undo-$name.journal
  rm -f "$journal"
  manifest "$V" >"$WORK/u.before.json"
  founder "$@" --journal "$journal"
  local code=$?
  manifest "$V" >"$WORK/u.after.json"
  founder undo "$journal"
  local undone=$?
  same "$V" "$WORK/u.before.json" "$fields" >"$WORK/u.diff"
  local back=$?
  founder redo "$journal"
  local redone=$?
  same "$V" "$WORK/u.after.json" "$fields" >>"$WORK/u.diff"
  local again=$?
  if ((code || undone || back || redone || again)); then
    BAD=$((BAD + 1))
    note "NOT $name: exits $code/$undone/$redone, $(head -2 "$WORK/u.diff" | tr '\n' ' ')"
  else
    note "$name ok"
  fi
}

t_undo_redo() {
  begin
  local other
  V=$(image undo APFS 512) && other=$(image undo2 APFS 256) || return
  mkdir -p "$V/A/M" "$V/B/M" "$V/C" "$other/X"
  $TREE tree "$V/A/T" --count 30 --max 30000 --seed 4
  $TREE tree "$V/A/T2" --count 30 --max 30000 --seed 6
  for name in doc name dup trash zip alias; do echo "$name" >"$V/A/$name.txt"; done
  echo old >"$V/B/doc.txt" && echo new >"$V/A/M/x" && echo old >"$V/B/M/x" && echo y >"$V/B/M/y"
  echo away >"$other/X/away.txt"
  undo_trip replace type,size,sha256 copy --conflict replace "$V/A/doc.txt" "$V/B"
  undo_trip merge type,size,sha256 copy --conflict merge "$V/A/M" "$V/B"
  undo_trip copy type,size,sha256 copy "$V/A/T" "$V/C"
  undo_trip move type,size,sha256 move "$V/A/T2" "$V/C"
  undo_trip move-volumes type,size,sha256 move "$other/X/away.txt" "$V/A"
  undo_trip rename type,size,sha256 rename "$V/A/name.txt" renamed.txt
  undo_trip duplicate type,size,sha256 duplicate "$V/A/dup.txt"
  undo_trip trash type,size,sha256 trash "$V/A/trash.txt"
  undo_trip new-folder type new-folder "$V/A" Fresh
  undo_trip compress type compress "$V/A/zip.txt"
  undo_trip alias type alias "$V/A/alias.txt"
  done_test undo_redo
}

t_trash_put_back() {
  begin
  local vol code back
  vol=$(image trash APFS 256) || return
  echo t >"$vol/t.txt" && mkdir "$vol/F" && echo f >"$vol/F/f.txt"
  founder trash "$vol/t.txt" "$vol/F"
  code=$?
  back=$(field results)
  note "trash exit $code into $(dirname "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])[0])' "$back")")"
  want "both left their folder" test ! -e "$vol/t.txt" -a ! -e "$vol/F"
  python3 -c 'import json,sys; print("\n".join(json.loads(sys.argv[1])))' "$back" >"$WORK/trashed"
  local items=()
  while IFS= read -r line; do items+=("$line"); done <"$WORK/trashed"
  founder put-back "${items[@]}"
  code=$?
  note "put back from the Trash's own record: exit $code $(errors | head -1)"
  want "put back restores both" is "$(cat "$vol/t.txt" "$vol/F/f.txt" 2>/dev/null | tr '\n' ,)" "t,f,"
  founder trash --journal "$WORK/trash.journal" "$vol/t.txt"
  founder undo "$WORK/trash.journal"
  want "undo puts back" is "$(cat "$vol/t.txt" 2>/dev/null)" t
  mkdir -p "$vol/D/sub" && echo a >"$vol/D/a" && echo b >"$vol/D/b" && echo c >"$vol/D/sub/c"
  ln -s b "$vol/D/link" && ln "$vol/D/a" "$vol/D/hl" && ln -s sub "$vol/D/slink"
  founder delete "$vol/D/link"
  founder delete "$vol/D/hl"
  founder delete "$vol/D/slink/"
  want "delete took only the named links" is "$(ls "$vol/D" | tr '\n' ,):$(cat "$vol/D/b" "$vol/D/a" "$vol/D/sub/c" | tr '\n' ,)" "a,b,sub,:b,a,c,"
  founder delete "$vol/D/sub"
  want "delete of a folder took only it" is "$(ls "$vol/D" | tr '\n' ,)" "a,b,"
  done_test trash_put_back
}

t_compress_staging() {
  begin
  local dir code
  dir=$(fresh comp)
  echo ok >"$dir/ok.txt" && echo secret >"$dir/secret.txt" && chmod 000 "$dir/secret.txt"
  founder compress "$dir/ok.txt" "$dir/secret.txt"
  code=$?
  note "exit $code: $(errors | head -1); left $(ls -A "$dir" | tr '\n' ' ')"
  want "a failed compress is an error" test "$code" -ne 0
  want "no staging folder left" none "$(count_temps "$dir")"
  want "no archive left" absent "$dir/Archive.zip"
  chmod 644 "$dir/secret.txt"
  founder compress "$dir/ok.txt" "$dir/secret.txt"
  want "the same compress works once readable" is "$?:$(unzip -l "$dir/Archive.zip" 2>/dev/null | grep -c -e ok.txt -e secret.txt)" "0:2"
  done_test compress_staging
}

# conc_round VOL ROUND JOBS KIND: JOBS runs started together, each bringing its own Set and same.txt.
conc_round() {
  local vol=$1 round=$2 jobs=$3 kind=$4
  local dst=$vol/R$round go=$WORK/conc.go i set text used='|' found=0 gone=0 pids=() codes=() zeros='' kept=''
  mkdir "$dst" && rm -f "$go"
  for ((i = 1; i <= jobs; i++)); do
    fresh "conc$i" >/dev/null
    $TREE tree "$WORK/conc$i/Set" --count 60 --max 200000 --seed $((round * 100 + i))
    text=$(printf '%s %d.%02d' "$kind" "$round" "$i")
    echo "$text" >"$WORK/conc$i/same.txt"
    manifest "$WORK/conc$i/Set" >"$WORK/conc$i.json"
    zeros+="0 " kept+="$text,"
  done
  for ((i = 1; i <= jobs; i++)); do
    (
      until [[ -e $go ]]; do sleep 0.01; done
      exec "$FOUNDER" --json "$kind" "$WORK/conc$i/Set" "$WORK/conc$i/same.txt" "$dst" >"$WORK/conc$i.jsonl" 2>&1
    ) &
    pids+=($!)
  done
  touch "$go"
  for ((i = 0; i < jobs; i++)); do
    wait "${pids[$i]}"
    codes+=($?)
    ((codes[i] == 0)) || note "round $round job $((i + 1)): $(errors "$WORK/conc$((i + 1)).jsonl" | head -1)"
  done
  note "round $round, $jobs ${kind}s: exits ${codes[*]}; landed $(ls "$dst" | tr '\n' ,)"
  want "round $round: every job exits 0" is "${codes[*]} " "$zeros"
  for ((i = 1; i <= jobs; i++)); do
    for set in "$dst"/Set*; do
      [[ $used == *"|$set|"* ]] && continue
      same "$set" "$WORK/conc$i.json" >/dev/null && { found=$((found + 1)) used+="$set|"; break; }
    done
  done
  want "round $round: every Set landed whole under its own name" \
    is "$found:$(ls -d "$dst"/Set* 2>/dev/null | wc -l | tr -d ' ')" "$jobs:$jobs"
  want "round $round: every same.txt kept" is "$(cat "$dst"/same*.txt 2>/dev/null | sort | tr '\n' ,)" "$kept"
  want "round $round: no temp left" none "$(count_temps "$dst")"
  if [[ $kind == move ]]; then
    for ((i = 1; i <= jobs; i++)); do
      absent "$WORK/conc$i/Set" && absent "$WORK/conc$i/same.txt" && gone=$((gone + 1))
    done
    want "round $round: every source gone" is "$gone" "$jobs"
  fi
}

t_concurrent() {
  begin
  local vol
  vol=$(image conc APFS 512) || return
  conc_round "$vol" 1 4 copy
  conc_round "$vol" 2 8 copy
  conc_round "$vol" 3 4 move
  done_test concurrent
}

# A move deletes a source only while it is what was copied: one written to after its copy stays.
t_changed_after_copy() {
  begin
  local src dst code landed
  src=$(fresh changed) && dst=$(image changed APFS 512) || return
  mkdir "$src/Tree"
  for i in $(seq 10 49); do head -c 200000 /dev/urandom >"$src/Tree/f$i.bin"; done
  head -c 150000000 /dev/urandom >"$src/Tree/zbig.bin"
  launch move --pause-after 5M "$src/Tree" "$dst"
  wait_for paused
  landed=$(cd "$dst/Tree" 2>/dev/null && ls | grep '^f' | head -1)
  [[ -n $landed ]] && echo "written after its copy" >>"$src/Tree/$landed"
  kill -USR1 "$PID"
  bounded 120 "$PID"
  code=$?
  note "appended to ${landed:-nothing} once it had landed: exit $code: $(errors "$WORK/bg.jsonl" | head -1)"
  want "a file that landed before the pause" test -n "$landed"
  want "the changed original kept" grep -q "written after its copy" "$src/Tree/$landed"
  want "the move says why" has "changed" "$WORK/bg.jsonl"
  done_test changed_after_copy
}

t_unmount() {
  begin
  local src vol code
  src=$(fresh unmount) && vol=$(image unmount APFS 1024) || return
  $TREE tree "$src/Tree" --count 60 --max 100000 --seed 9 --big big.bin:300000000
  manifest "$src/Tree" >"$WORK/unmount.json"
  launch move --pause-after 40M "$src/Tree" "$vol"
  wait_for paused
  hdiutil detach -quiet -force "$vol"
  kill -USR1 "$PID"
  bounded 120 "$PID"
  code=$?
  note "exit $code: $(errors "$WORK/bg.jsonl" | head -2 | tr '\n' ' ')"
  want "a failure, not success or a hang" test "$code" -eq 1 -o "$code" -eq 2
  want "a reason given" test -n "$(errors "$WORK/bg.jsonl")"
  want "every source intact" same "$src/Tree" "$WORK/unmount.json" >/dev/null
  done_test unmount
}

t_big_tree() {
  begin
  local src vol code rss secs gap
  src=$(fresh big) && vol=$(image big APFS 2048) || return
  $TREE tree "$src/Big" --count 20000 --min 100 --max 4000 --depth 3 --fanout 12 --seed 3
  manifest "$src/Big" >"$WORK/big.json"
  /usr/bin/time -l "$FOUNDER" --json copy "$src/Big" "$vol" >"$WORK/big.jsonl" 2>"$WORK/big.time"
  code=$?
  rss=$(awk '/maximum resident set size/ {print int($1 / 1048576)}' "$WORK/big.time")
  secs=$(awk '/real/ {print $1; exit}' "$WORK/big.time")
  gap=$(python3 - "$WORK/big.jsonl" <<'PY'
import json, sys
last, gap, done = 0.0, 0.0, -1
for line in open(sys.argv[1]):
    event = json.loads(line)
    if event.get("event") == "progress" and event["items_done"] != done:
        gap, last, done = max(gap, event["secs"] - last), event["secs"], event["items_done"]
print(round(gap, 2))
PY
)
  note "copy of 20000 files: exit $code in ${secs}s, peak ${rss} MB, longest stall ${gap}s"
  want "copy exits 0" is "$code" 0
  want "every file copied whole" same "$vol/Big" "$WORK/big.json" >/dev/null
  want "memory stays under 200 MB" test "${rss:-999}" -lt 200
  want "no stall over 3 s" python3 -c "import sys; sys.exit(float('$gap') > 3)"
  mkdir "$vol/m"
  /usr/bin/time -l "$FOUNDER" --json move "$src/Big" "$vol/m" >"$WORK/big.jsonl" 2>"$WORK/big.time"
  code=$?
  rss=$(awk '/maximum resident set size/ {print int($1 / 1048576)}' "$WORK/big.time")
  secs=$(awk '/real/ {print $1; exit}' "$WORK/big.time")
  note "move: exit $code in ${secs}s, peak ${rss} MB"
  want "move exits 0" is "$code" 0
  want "every file moved whole" same "$vol/m/Big" "$WORK/big.json" >/dev/null
  want "the source tree gone" absent "$src/Big"
  done_test big_tree
}

# Asks Finder over AppleScript for the names it gives, where founder should give the same.
t_finder() {
  begin
  osascript -e 'tell application "Finder" to get name of startup disk' >/dev/null 2>&1 &
  if ! bounded 20 $!; then
    record finder skip "Finder does not take Apple Events from this session; grant Automation to compare"
    return
  fi
  local vol a b long
  vol=$(image finder APFS 128) || return
  long=$(printf 'y%.0s' {1..251}).txt
  for side in finder founder; do
    mkdir -p "$vol/$side"
    echo a >"$vol/$side/a.txt" && echo long >"$vol/$side/$long" && ln -s a.txt "$vol/$side/link"
  done
  osascript -e "tell application \"Finder\" to duplicate (POSIX file \"$vol/finder/a.txt\" as alias)" \
    -e "tell application \"Finder\" to duplicate (POSIX file \"$vol/finder/$long\" as alias)" >/dev/null 2>&1
  "$FOUNDER" duplicate "$vol/founder/a.txt" "$vol/founder/$long" >/dev/null 2>&1
  a=$(ls "$vol/finder" | sort | tr '\n' '/')
  b=$(ls "$vol/founder" | sort | tr '\n' '/')
  note "Finder made $(ls "$vol/finder" | sort | sed -n 3p | head -c 60)..."
  want "duplicate names match Finder's" is "$a" "$b"
  done_test finder
}

t_search() {
  begin
  local dir code
  dir=$(fresh search)
  mkdir -p "$dir/Docs/inner"
  echo x >"$dir/Docs/zebra-quartz-$$.txt" && echo yy >"$dir/Docs/inner/two.txt"
  founder search --index "$WORK/index" --home "$dir" "zebra-quartz-$$"
  code=$?
  want "search finds the file" grep -q "zebra-quartz-$$.txt" "$WORK/last.jsonl"
  founder totals --index "$WORK/index" --home "$dir" "$dir/Docs"
  note "search exit $code; totals: $(grep '"event":"totals"' "$WORK/last.jsonl" | head -c 160)"
  want "totals count the folder" grep -q '"items":3' "$WORK/last.jsonl"
  rm -rf "$WORK/index"
  done_test search
}

TESTS=(poll_move kill_move kill_copy kill_deep kill_cleanup cancel full_upfront full_midway read_only
  locked unreadable acl_denied into_itself same_file symlinks case_names unicode odd_names path_max
  metadata fat_litter fat32_4gb conflicts undo_redo trash_put_back compress_staging concurrent
  changed_after_copy unmount big_tree finder search)
((${#ONLY[@]})) && TESTS=("${ONLY[@]}")

say "work: $WORK"
say "founder: $FOUNDER"
for test in "${TESTS[@]}"; do
  say "== $test"
  recorded=${#RESULTS[@]}
  "t_$test"
  ((${#RESULTS[@]} > recorded)) || record "$test" fail "its fixtures could not be set up"
  detach_all
  for scratch in "$WORK"/*; do
    case ${scratch##*/} in mnt | images | *.json | *.jsonl | *.err | *.journal | *.report | *.split | *.diff) ;; *)
      unlock "$scratch"
      rm -rf "$scratch"
      ;;
    esac
  done
done
table
