
echo
echo "--- Cleaning ---"
for entry in "${plan[@]}"; do
  label="${entry%%|*}"; rest="${entry#*|}"; rest="${rest#*|}"
  printf '  %-40s cleaning... ' "$label"
  IFS=':' read -r -a paths <<< "$rest"
  for p in "${paths[@]}"; do
    [ -e "$p" ] && rm -rf "$p" 2>/dev/null
  done
  echo done
done

if command -v apt-get >/dev/null 2>&1; then
  echo
  echo "--- apt clean / autoremove ---"
  sudo apt-get -y clean   >/dev/null 2>&1 || true
  sudo apt-get -y autoclean >/dev/null 2>&1 || true
fi

if command -v journalctl >/dev/null 2>&1; then
  sudo journalctl --vacuum-time=2d >/dev/null 2>&1 || true
fi
