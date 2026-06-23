#!/bin/bash
# Diagnostic instrumentation (AZL4 rollback generator-freeze investigation).
#
# Wraps selected systemd generators with strace so that a PID1
# generator-phase freeze reveals the exact blocking syscall on the serial
# console. strace runs line-buffered (stdbuf -oL -eL) and writes to stderr,
# which systemd forwards to the kernel console; this survives the SIGALRM/
# SIGKILL that tears down the generator sandbox on timeout, so the last
# syscall before the freeze is preserved. The real generator is stashed
# outside the generators directory and re-exec'd with argv[0] preserved
# (required so netplan still detects generator mode via its argv[0] check).
set -euo pipefail
GENDIR=/usr/lib/systemd/system-generators
STASH=/usr/libexec/gen-real
mkdir -p "$STASH"

wrap() {
  local name="$1" src="$GENDIR/$1" real
  [ -e "$src" ] || { echo "strace-generators: skip $name (absent)"; return; }
  real="$(readlink -f "$src")"
  cp -a "$real" "$STASH/$name.real"
  rm -f "$src"
  cat > "$src" <<EOF
#!/bin/bash
exec stdbuf -oL -eL strace -f -tt -T -s 512 \
  /bin/bash -c 'exec -a "$src" "$STASH/$name.real" "\$@"' _ "\$@"
EOF
  chmod 755 "$src"
  echo "strace-generators: wrapped $name -> $STASH/$name.real"
}

wrap netplan
wrap systemd-import-generator
wrap systemd-system-update-generator
