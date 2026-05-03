# Linux guest disk cleanup. Same shape as the Windows variant — categorized
# scan, du-driven sizes. The Rust caller substitutes __DEEP_BLOCK__ (extra
# deep-mode targets) and __ACTION__ (the actual delete pass — empty in
# dry-run mode).

set +e
fmt() { b=$1; if [ "$b" -ge 1073741824 ]; then awk -v b="$b" 'BEGIN{printf "%.1f GB",b/1073741824}'; elif [ "$b" -ge 1048576 ]; then awk -v b="$b" 'BEGIN{printf "%d MB",b/1048576}'; elif [ "$b" -ge 1024 ]; then awk -v b="$b" 'BEGIN{printf "%d KB",b/1024}'; else echo "$b B"; fi; }
bytes() {
  total=0
  IFS=':' read -r -a paths <<< "$1"
  for p in "${paths[@]}"; do
    [ -e "$p" ] || continue
    sz=$(du -sb "$p" 2>/dev/null | awk '{print $1}')
    [ -n "$sz" ] && total=$((total + sz))
  done
  echo "$total"
}

# label|path1[:path2[:...]]
targets=(
  'utm-dev build/run logs|'$HOME'/.utm-dev-build:'$HOME'/.utm-dev-run:/tmp/utm-dev-*'
  'apt cache|/var/cache/apt/archives'
  'journal logs|/var/log/journal'
  'old crash reports|/var/crash'
  'temp files (>2 days old)|/tmp'__DEEP_BLOCK__
)

before_kb=$(df --output=avail / | tail -n 1 | tr -d ' ')
before=$((before_kb * 1024))
echo "/ free before: $(fmt $before)"
echo
echo "--- Scanning ---"
plan=()
for t in "${targets[@]}"; do
  label="${t%%|*}"; paths="${t#*|}"
  b=$(bytes "$paths")
  printf '  %-40s %s\n' "$label" "$(fmt $b)"
  [ "$b" -gt 0 ] && plan+=("$label|0|$paths")
done
__ACTION__

after_kb=$(df --output=avail / | tail -n 1 | tr -d ' ')
after=$((after_kb * 1024))
freed=$((after - before))
echo
echo "Freed:   $(fmt $freed)"
echo "/ free:  $(fmt $before) -> $(fmt $after)"
