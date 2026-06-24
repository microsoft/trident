#!/bin/bash
# Diagnostic instrumentation (AZL4 rollback generator-freeze investigation).
#
# Wraps selected systemd generators with strace so that a PID1
# generator-phase freeze reveals the exact blocking syscall.
#
# Output goes to /dev/kmsg, NOT stderr: systemd runs generators inside a
# sandbox (FORK_NEW_MOUNTNS|FORK_MOUNTNS_SLAVE) where the child's stderr is
# effectively discarded -- the generator log lines that reach the serial
# console do so via systemd's already-open kmsg channel. Writing strace
# output to /dev/kmsg is therefore the only route that survives to the
# serial log. strace runs line-buffered (stdbuf -oL -eL) so each completed
# syscall line is flushed immediately and survives the SIGALRM/SIGKILL that
# tears down the generator sandbox when the 90s timeout fires; the last
# line before the freeze names the blocking syscall.
#
# The real generator is stashed outside the generators directory and
# re-exec'd with argv[0] preserved (required so netplan still detects
# generator mode via its argv[0] check).
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
exec 2>/dev/kmsg
echo "strace-gen[$name] begin pid \$\$" >&2
exec stdbuf -oL -eL strace -f -tt -T -s 512 \
  /bin/bash -c 'exec -a "$src" "$STASH/$name.real" "\$@"' _ "\$@"
EOF
  chmod 755 "$src"
  echo "strace-generators: wrapped $name -> $STASH/$name.real"
}

wrap netplan
wrap systemd-import-generator
wrap systemd-system-update-generator
