# Trident SELinux Permission Audit Report

This document audits every permission in `trident.te` that depends on types from
external SELinux modules which are candidates for removal during ACL policy
minimization. For each permission, it provides **concrete trident source code**
proving the permission is directly exercised by the trident binary, or documents
that the permission is **indirect** (exercised by a subprocess like dracut that
inherits `trident_t`), or flags it as **unsubstantiated** (no code path found).

---

## Background

Trident's SELinux policy (`packaging/selinux-policy-trident/trident.te`) declares
~180 external type dependencies in its `require {}` block. During ACL image
builds, `rpm_configure_selinux()` in `rpm_install.sh` removes unused SELinux
modules via `semodule -X 100 -r`. If a removed module defines a type that
trident's policy hard-requires, the policy rebuild fails.

### Key architectural facts

1. **Trident runs as `trident_t`** and all subprocesses that lack a domain
   transition inherit this domain.
2. **`setfiles -m`** runs as `setfiles_t` via `seutil_run_setfiles(trident_t,
   system_r)` — a domain transition occurs, so setfiles operations do NOT need
   `trident_t` permissions.
3. **`dracut`** runs as `trident_t` (no `dracut_t` in refpolicy) and internally
   uses `cp -a`, which preserves SELinux xattrs and triggers `relabelfrom`/
   `relabelto` on `trident_t`.
4. **`udevadm`** runs as `udevadm_t` via domain transition — rules for
   `udevadm_t` are separate from `trident_t`.
5. **All external tools** trident invokes are declared in the `Dependency` enum
   (`crates/osutils/src/dependencies.rs`). This enum does NOT include `Sudo`,
   `Ntpd`, `Chronyd`, `Kadmind`, `Gpg`, or `GpgAgent`.

### How to verify a permission is used

A permission is **directly used** if:
- The trident `Dependency` enum includes the tool, AND
- Rust code calls `Dependency::X.cmd()...run_and_check()` in a non-test path

A permission is **indirectly used** if:
- A subprocess (dracut, RPM scriptlet) exercises it while running as `trident_t`
  or `rpm_script_t`

A permission is **unsubstantiated** if:
- No call site exists in trident source or its subprocesses
- Likely added via `audit2allow` during development and never cleaned up

### Classification key

> 🟢 **DIRECT** — Trident source code directly uses this capability (code excerpts provided)
>
> 🔵 **INDIRECT (dracut)** — Permission exercised by dracut running as `trident_t`: either `cp -a` triggering `relabelto`, or dracut module `check()` functions calling `require_binaries` → `find_binary` → `stat()` triggering `getattr`
>
> 🔴 **UNSUBSTANTIATED** — No code path found in trident or dracut; likely from `audit2allow`
>
> ⚪ **RPM INSTALL** — Exercised by the package manager during RPM install/upgrade, not by trident itself

---

## 📦 Module: `cgroup`

### Types: `cgroup_t`, `memory_pressure_t`

#### ➤ `allow trident_t cgroup_t:filesystem getattr` — 🟢 DIRECT

> Allows trident to `statfs()` the cgroup filesystem when enumerating mounts via `findmnt`.

Trident enumerates all mounted filesystems including `/sys/fs/cgroup` via
`findmnt --json`, which does `statfs()` on each mount:

```rust
// crates/trident/src/diagnostics.rs:195-197
let mount_info = FindMnt::run()
    .map_err(|e| record_failure(failures, "mount info", &e))
    .ok();
```

`FindMnt::run()` is defined in `crates/osutils/src/findmnt.rs:143-178`:
```rust
impl FindMnt {
    pub fn run() -> Result<Self, Error> {
        Self::run_cmd(&mut Self::build_cmd())
    }

    fn build_cmd() -> TridentCommand {
        let mut cmd = Dependency::Findmnt.cmd();
        cmd.arg("--json").arg("-o").arg(FINDMNT_COLUMNS);
        cmd
    }
}
```

`Dependency::Findmnt` is in the enum at `dependencies.rs:100`. `FindMnt::run()`
is called from:
- `crates/trident/src/diagnostics.rs:195` — host diagnostics collection
- `crates/trident/src/engine/newroot.rs:485` — newroot pivot operations
- `crates/trident/src/subsystems/selinux.rs:261` — `filesystems_to_relabel()`

Tests verify cgroup visibility:
```rust
// crates/osutils/src/findmnt.rs:390-391
assert!(root.contains_mountpoint("/sys/fs/cgroup"));
```

#### ➤ `create_files_pattern(trident_t, cgroup_t, cgroup_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to create files under `/sys/fs/cgroup/` — a systemd-managed hierarchy.

Expands to:
```
allow trident_t cgroup_t:dir add_entry_dir_perms;
allow trident_t cgroup_t:file create_file_perms;
```

**Source search result:** Zero matches for cgroup file creation in Rust source.
No code creates files under `/sys/fs/cgroup`. Systemd manages cgroup hierarchy,
not trident.

#### ➤ `allow trident_t memory_pressure_t:file { read open getattr setattr }` — 🔴 UNSUBSTANTIATED

> Would allow trident to read PSI (Pressure Stall Information) memory pressure files.

Also covered by macro at line 687:
```
fs_watch_memory_pressure(trident_t)
```
which expands to (from `/usr/share/selinux/devel/include/kernel/filesystem.if`):
```
allow $1 memory_pressure_t:file { rw_file_perms setattr };
# temp workaround until labeling issues are resolved.
allow $1 cgroup_t:file { rw_file_perms setattr };
```

**Source search result:** `grep -ri 'memory.pressure\|/proc/pressure\|memory_pressure' crates/`
returns **zero matches**. No trident code reads PSI (Pressure Stall Information)
files. The upstream macro itself is labeled as a "temp workaround."

#### ➤ `allow udevadm_t cgroup_t:filesystem getattr` + `read_lnk_files_pattern(...)` — 🟢 DIRECT

> Allows `udevadm` (running as `udevadm_t`) to traverse `/sys` cgroup symlinks during device settling.

Trident invokes `udevadm` which transitions to `udevadm_t`. These rules let
`udevadm_t` traverse `/sys` (which contains cgroup symlinks):

```rust
// crates/osutils/src/udevadm.rs:7-13
pub fn settle() -> Result<(), Error> {
    Dependency::Udevadm
        .cmd()
        .arg("settle")
        .run_and_check()
        .context("Failed settle udev setup")
}
```

`Dependency::Udevadm` is in the enum at `dependencies.rs:139`. Call sites:
- `crates/osutils/src/encryption.rs:402` — after LUKS setup
- `crates/osutils/src/mkfs.rs:235` — after filesystem creation

---

## 📦 Module: `gpg`

### Types: `gpg_agent_exec_t`, `gpg_pinentry_exec_t`, `gpg_secret_t`

#### ➤ `domain_entry_file(trident_t, gpg_agent_exec_t)` — 🔴 UNSUBSTANTIATED

> Would make `gpg-agent` an entry point for the `trident_t` domain — architecturally suspect.

Expands to:
```
allow trident_t gpg_agent_exec_t:file entrypoint;
allow trident_t gpg_agent_exec_t:file { mmap_exec_file_perms ioctl lock };
typeattribute gpg_agent_exec_t entry_type;
```

This makes `gpg_agent_exec_t` an **entry point** for `trident_t` — meaning
the trident domain could be entered by executing the gpg-agent binary. This
is architecturally suspect.

**Source search result:** `grep -ri 'gpg\|gpg-agent\|gpg_agent' crates/**/*.rs`
returns only:
- Test data: `crates/trident_api/src/samples/sample_hc.rs` — sudoers config
- Package metadata: `crates/trident/src/osimage/cosi/metadata.rs:678` — `"gpg-pubkey"` RPM name

**No `Dependency::Gpg*` exists** in the enum (`dependencies.rs:91-150`).
No code invokes `gpg`, `gpg-agent`, `gpg2`, or `gpgconf`.

The Go installer has a comment about gpg-agent cleanup:
```go
// tools/installer/internal/shell/shell.go:45-46
// this will block the gpg-agent cleanup mechanism from running,
// which may cause chroots to not unmount properly.
```
But the Go installer is a separate binary, not `trident_t`.

#### ➤ `gpg_entry_type(trident_t)` — 🔴 UNSUBSTANTIATED

> Would make `gpg` an entry point for `trident_t`, allowing domain entry via the GPG binary.

Expands to `domain_entry_file(trident_t, gpg_exec_t)`. Same analysis — no
GPG invocations in trident.

#### ➤ `gpg_list_user_secrets(trident_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to list directories labeled `gpg_secret_t` (e.g. `~/.gnupg/`).

Expands to listing `gpg_secret_t` directories. No code reads `~/.gnupg/`.

#### ➤ `allow trident_t gpg_pinentry_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` the pinentry binary.

#### ➤ `allow trident_t gpg_secret_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` GPG secret key files.

No trident code traverses directories containing these files. No dracut module
checks for `pinentry` or reads `~/.gnupg/`. The `91crypt-gpg` dracut module
checks for `gpg` but not `pinentry`. These were likely captured by `audit2allow`
during development.

---

## 📦 Module: `sudo`

### Type: `sudo_exec_t`

#### ➤ `can_exec(trident_t, sudo_exec_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to execute `/usr/bin/sudo` — unnecessary since trident always runs as root.

Expands to:
```
allow trident_t sudo_exec_t:file { mmap_exec_file_perms ioctl lock execute_no_trans };
```

**Source search result:** `grep -ri 'sudo' crates/**/*.rs` returns only:
- `crates/trident_api/src/samples/sample_hc.rs:1289,1448,1630` — sudoers
  config **inside host configuration JSON samples** (data, not code)

**No `Dependency::Sudo` exists** in the enum.

Trident requires root directly and verifies at startup:
```rust
// crates/trident/src/lib.rs:230-233
if !Uid::effective().is_root() {
    // ... return CheckRootPrivileges error
}
```

Every external tool is invoked directly via `Dependency::X.cmd()`:
```rust
Dependency::Dracut.cmd()     // mkinitrd.rs:74
Dependency::Setfiles.cmd()   // selinux.rs:239
Dependency::Systemctl.cmd()  // systemd.rs:37, many others
Dependency::Udevadm.cmd()    // udevadm.rs:8
```

All systemd units run trident directly:
```ini
# packaging/systemd/trident.service
ExecStart=trident commit

# packaging/systemd/tridentd.service
ExecStart=trident daemon
```

`sudo` usage in the repo is exclusively in test/CI tooling that does NOT run
as `trident_t`:
- `scripts/loop-update/servicing-tests.sh`
- `tests/functional_tests/tools/trident.py`
- `tests/e2e_tests/helpers/ssh_utilities.py`

---

## 📦 Module: `ntp`

### Types: `ntpd_exec_t`, `ntpd_unit_t`

#### ➤ `allow trident_t ntpd_exec_t:file { execute getattr }` — 🔴 UNSUBSTANTIATED (execute)

> Would allow trident to execute the NTP daemon binary (also covers `systemd-timesyncd`/`systemd-timedated` via `file_contexts`).

Most `*_exec_t` types only get `getattr`. This one has **execute**, meaning
someone expected trident to run the NTP daemon binary.

**Source search result:** `grep -ri 'chronyd\|chrony\|ntpd\|ntp_sync' crates/**/*.rs`
returns **zero matches**. No `Dependency::Ntpd` or `Dependency::Chronyd` exists.

The only time-sync evidence is in the SELinux policy itself:
```
# trident.te:611-613
chronyd_exec(trident_t)
chronyd_read_config(trident_t)
chronyd_read_key_files(trident_t)
```
These macros grant chrony permissions, but no Rust code invokes chrony.

The `chrony` package is in the image package list
(`crates/trident/src/init/offline/aksee_prism_history.json:189`), but that
only means chrony is installed in the OS, not that trident manages it.

The `execute` on `ntpd_exec_t` is LEGACY — NTP was replaced by chrony,
and neither is invoked from trident code.

#### ➤ `allow trident_t ntpd_unit_t:file getattr` — 🔵 INDIRECT (dracut)

> Allows `stat()` on NTP-related systemd unit files, triggered by dracut module enumeration.

The `ntpd_exec_t` type label covers `/usr/lib/systemd/systemd-timedated` and
`/usr/lib/systemd/systemd-timesyncd` (per `file_contexts`). Dracut modules
`01systemd-timedated` and `01systemd-timesyncd` call `require_binaries` on
these paths in their `check()` functions:

```bash
# /usr/lib/dracut/modules.d/01systemd-timesyncd/module-setup.sh
check() {
    require_binaries \
        "$systemdutildir"/systemd-timesyncd \
        "$systemdutildir"/systemd-time-wait-sync \
        || return 1
    return 255
}
```

`require_binaries` → `find_binary` → `test -x` performs a `stat()` syscall on
these paths. Since dracut runs as `trident_t`, the `stat()` triggers
`getattr` denials on `ntpd_exec_t` (which covers the timesyncd/timedated binaries).

The `ntpd_unit_t` getattr similarly comes from dracut's `01systemd-timesyncd`
`install()` function, which copies unit files using `inst_multiple`, triggering
`stat()` on systemd time-related unit files.

---

## 📦 Module: `chronyd`

### Types: `chronyc_exec_t`, `chronyd_unit_t`, `chronyd_var_lib_t`, `chronyd_var_log_t`

#### ➤ `chronyd_exec(trident_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to execute `/usr/bin/chronyd` — the chrony time daemon.

Expands to `can_exec(trident_t, chronyd_exec_t)` — full execute permission.

**Source search result:** No `Dependency::Chronyd` in the enum. Zero calls to
chronyd in Rust source. The macros `chronyd_read_config(trident_t)` and
`chronyd_read_key_files(trident_t)` alongside `chronyd_exec` suggest these were
added anticipating chrony management, but the feature was never implemented.

#### ➤ `chronyd_read_config(trident_t)` + `chronyd_read_key_files(trident_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to read `/etc/chrony.conf` and `/etc/chrony.keys`.

Same analysis — no code reads chrony config files.

#### ➤ `allow trident_t chronyc_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` `/usr/bin/chronyc` (the chrony CLI client).

No trident code stats `/usr/bin/chronyc`. No dracut module calls
`require_binaries chronyc`. Likely captured by `audit2allow`.

#### ➤ `allow trident_t chronyd_unit_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` chronyd systemd unit files.

No trident code stats chronyd unit files (`/usr/lib/systemd/system/*chronyd*`).
No dracut module checks for chronyd services. Likely captured by `audit2allow`.

#### ➤ `allow trident_t chronyd_var_lib_t:dir { getattr open read relabelto }` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to traverse and relabel `/var/lib/chrony/` during initramfs generation.

All four permissions are triggered by dracut's `cp -a` when building initramfs.
The `cp -a` command calls `stat()` (`getattr`), `opendir()`/`readdir()`
(`open read`), and restores SELinux xattrs (`relabelto`) on `/var/lib/chrony/`.
See [Dracut Chain appendix](#appendix-the-dracut-chain).

#### ➤ `allow trident_t chronyd_var_log_t:dir { getattr open read relabelto }` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to traverse and relabel `/var/log/chrony/` during initramfs generation.

Same as above for `/var/log/chrony/`.

---

## 📦 Module: `dhcp`

### Types: `dhcpc_exec_t`, `dhcpc_state_t`, `dhcpd_unit_t`

#### ➤ `allow trident_t dhcpc_exec_t:file getattr` — 🔵 INDIRECT (dracut)

> Allows `stat()` on `/usr/sbin/dhclient`, triggered by dracut's `35network-legacy` module check.

Dracut module `35network-legacy` calls `require_binaries ip dhclient` in its
`check()` function:

```bash
# /usr/lib/dracut/modules.d/35network-legacy/module-setup.sh
check() {
    require_binaries ip dhclient sed awk grep pgrep tr expr || return 1
    require_any_binary arping arping2 || return 1
    return 255
}
```

`require_binaries dhclient` → `find_binary "dhclient"` → `test -x /usr/sbin/dhclient`
performs a `stat()` syscall. Since `/usr/sbin/dhclient` is labeled `dhcpc_exec_t`
and dracut runs as `trident_t`, this triggers the `getattr` denial.

#### ➤ `allow trident_t dhcpc_state_t:dir { getattr open read relabelto }` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to traverse and relabel DHCP state dirs (`/var/lib/dhclient/`, `/var/lib/dhcpcd/`).

All permissions from dracut's `cp -a` operating on `/var/lib/dhclient/` and
`/var/lib/dhcpcd/`. The `cp -a` calls `stat()` (`getattr`), reads the directory
(`open read`), and restores SELinux xattrs (`relabelto`).
See [Dracut Chain appendix](#appendix-the-dracut-chain).

#### ➤ `allow trident_t dhcpd_unit_t:file getattr` — 🔴 UNSUBSTANTIATED (already optional)

> Would allow trident to `stat()` dhcpd systemd unit files. Already wrapped in `optional_policy`.

No trident code or dracut module stats dhcpd unit files. Already in
`optional_policy` (lines 560-566), so harmless.

#### ➤ `sysnet_read_dhcp_config(trident_t)` — 🟢 DIRECT (indirect via netplan)

> Allows trident to read DHCP configuration files (`dhcp_etc_t`), needed for netplan-based network setup.

Grants access to `dhcp_etc_t` files. Trident manages network configuration
via netplan, which generates DHCP configuration:

```rust
// crates/trident/src/subsystems/network.rs:50-54
match ctx.spec.os.netplan.as_ref() {
    Some(config) => {
        debug!("Configuring network");
        netplan::write(config).structured(ServicingError::WriteNetplanConfig)?;
        netplan::generate().structured(ServicingError::GenerateNetplanConfig)?;
```

`netplan::generate()` shells out to `netplan generate` which reads/writes
network config including DHCP settings:
```rust
// crates/osutils/src/netplan.rs:43
Dependency::Netplan.cmd().arg("generate").run_and_check()?;
```

`Dependency::Netplan` is in the enum at `dependencies.rs:113`. Netplan
config includes DHCP:
```rust
// crates/osutils/src/netplan.rs:126
dhcp4: Some(true),
```

Trident also has a dedicated network provisioning path:
```rust
// crates/trident/src/engine/provisioning_network.rs:33
fn start_provisioning_network(config: &NetworkConfig, ...) -> Result<(), Error> {
    netplan::write(config).context("Failed to write provisioning netplan config")?;
    netplan::apply().context("Failed to apply provisioning netplan config")?;
```

---

## 📦 Module: `kerberos`

### Type: `krb5kdc_exec_t` (in require block); `kadmind_exec_t`, `krb5_conf_t` (via macros)

#### ➤ `allow trident_t krb5kdc_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` `/usr/sbin/krb5kdc` (the Kerberos KDC daemon).

No trident code stats `/usr/sbin/krb5kdc`. No dracut module calls
`require_binaries krb5kdc`. Likely captured by `audit2allow`.

#### ➤ `kerberos_exec_kadmind(trident_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to execute the Kerberos admin daemon (`kadmind`).

Expands to `can_exec(trident_t, kadmind_exec_t)` — full execute on the
Kerberos admin daemon.

**Source search result:** No `Dependency::Kadmind` in the enum. Zero
references to `kadmind`, `kinit`, `klist`, or any Kerberos tool in Rust source.

#### ➤ `kerberos_read_config(trident_t)` — 🔴 UNSUBSTANTIATED

> Would allow trident to read `/etc/krb5.conf` (Kerberos configuration).

Grants reading `krb5_conf_t` (`/etc/krb5.conf`). No trident code reads this
file. No dracut module accesses Kerberos config. Likely from `audit2allow`.

---

## 📦 Module: `logrotate`

### Types: `logrotate_unit_t`, `logrotate_var_lib_t`

#### ➤ `allow trident_t logrotate_unit_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` logrotate systemd unit files.

No trident code or dracut module stats logrotate unit files. Likely from `audit2allow`.

#### ➤ `allow trident_t logrotate_var_lib_t:dir { getattr open read relabelto }` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to traverse and relabel `/var/lib/logrotate/` during initramfs generation.

The `relabelto` is dracut's `cp -a`.

---

## 📦 Module: `oddjob`

### Type: `oddjob_mkhomedir_exec_t`

#### ➤ `allow trident_t oddjob_mkhomedir_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` `/usr/libexec/oddjob/mkhomedir` (PAM home directory creator).

No trident code or dracut module stats `/usr/libexec/oddjob/mkhomedir`.
Likely from `audit2allow`.

---

## 📦 Module: `slocate`

### Type: `locate_exec_t`

#### ➤ `allow trident_t locate_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` `/usr/bin/locate` (the slocate/mlocate file finder).

No trident code or dracut module stats `/usr/bin/locate`. Likely from
`audit2allow`.

---

## 📦 Module: `uuidd`

### Types: `uuidd_exec_t`, `uuidd_var_lib_t`

#### ➤ `allow trident_t uuidd_exec_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` `/usr/sbin/uuidd` (the UUID generation daemon).

No trident code or dracut module stats `/usr/sbin/uuidd`. Likely from
`audit2allow`.

#### ➤ `allow trident_t uuidd_var_lib_t:dir relabelto` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to relabel `/var/lib/uuidd/` during initramfs generation.

Pure `relabelto` — only triggered by dracut's `cp -a`.

#### ➤ `uuidd_manage_lib_dirs(trident_t)` (line 810) — 🔴 UNSUBSTANTIATED

> Would allow trident to create, remove, and manage directories under `/var/lib/uuidd/`.

Grants full manage permissions on `/var/lib/uuidd/`. No `Dependency::Uuidd`
in the enum. No code references uuidd.

---

## 📦 Module: `rpm`

### Types: `rpm_t`, `rpm_script_t`, `rpm_unit_t`, `rpm_var_lib_t`, `rpm_var_cache_t`

#### ➤ `allow trident_t rpm_unit_t:file getattr` — 🔴 UNSUBSTANTIATED

> Would allow trident to `stat()` RPM-related systemd unit files.

No trident code or dracut module stats RPM unit files. Likely from `audit2allow`.

#### ➤ `allow trident_t rpm_var_lib_t:dir { add_name relabelto }` + `file { ... }` — 🔵🔴 INDIRECT (dracut) + UNSUBSTANTIATED

> The `relabelto` is dracut's `cp -a` on `/var/lib/rpm/`; the write perms would let trident modify the RPM database.

The `relabelto` components are dracut's `cp -a`. The `add_name create setattr
write` components are **unsubstantiated** — no trident code writes to the RPM
database. No `Dependency::Rpm` or `Dependency::Tdnf` exists in the enum.

#### ➤ `allow trident_t rpm_var_cache_t:dir relabelto` + `file relabelto` — 🔵 INDIRECT (dracut)

> Allows dracut's `cp -a` to relabel `/var/cache/rpm/` during initramfs generation.

Pure `relabelto` on `/var/cache/rpm/`.

#### ➤ Rules for `rpm_t` and `rpm_script_t` — ⚪ NOT A TRIDENT PERMISSION

> These extend `rpm_t`/`rpm_script_t` domains — exercised by the package manager, not by trident.

```
#============= rpm_t ==============
allow rpm_t unlabeled_t:dir { add_name getattr remove_name search write };
allow rpm_t unlabeled_t:file { create getattr ioctl open read rename write };
allow rpm_t rpm_script_t:process { noatsecure rlimitinh siginh };

#============= rpm_script_t ==============
# Allow RPM scripts to read SELinux policy
# (we currently apply trident.pp as a module in the Trident spec)
allow rpm_script_t security_t:security read_policy;
allow rpm_script_t kernel_t:fd use;
allow rpm_script_t unlabeled_t:dir { add_name getattr remove_name search write };
allow rpm_script_t unlabeled_t:file { create getattr ioctl open read rename write };
```

> **Important:** These are NOT permissions for `trident_t`. They extend
> `rpm_t` and `rpm_script_t` — the domains that run when **any package manager**
> (tdnf, rpm, dnf) installs or upgrades the `trident-selinux` RPM. Trident
> itself never exercises these permissions.

The trident RPM spec triggers them via `%post`:

```spec
# packaging/rpm/trident.spec:185-186
%post selinux
%selinux_modules_install -s %{selinuxtype} %{_datadir}/selinux/packages/%{selinuxtype}/%{name}.pp.bz2
```

The `%post selinux` scriptlet runs `semodule -i` as `rpm_script_t`. It needs:
- `security_t:security read_policy` — to read current policy during module install
- `unlabeled_t` access — files may lack labels before first restorecon
- `kernel_t:fd use` — inherit file descriptors during installation

These rules are bundled into `trident.te` because the trident SELinux module
is the right place to declare what extra permissions the RPM install process
needs when handling trident's policy. They are required for the RPM lifecycle
to work, but the actor is the package manager, not the trident daemon.

**Verdict:** Required for RPM install/upgrade, but not a trident runtime
permission. These should remain but are architecturally distinct from everything
else in this report.

---

## 📦 Module: `cloudinit` (already in `optional_policy`)

### Types: `cloud_init_t`, `cloud_init_exec_t`, `cloud_init_state_t`

Already wrapped in `optional_policy` blocks (lines 533-543, 641-647, 856-868,
946-952).

**Evidence for cloud-init interaction (DIRECT):**

Trident's network subsystem directly interacts with cloud-init configuration:

```rust
// crates/trident/src/subsystems/network.rs:14-16
const CLOUD_INIT_CONFIG_DIR: &str = "/etc/cloud/cloud.cfg.d";
const CLOUD_INIT_DISABLE_FILE: &str = "99-use-trident-networking.cfg";
const CLOUD_INIT_DISABLE_CONTENT: &str = "network: {config: disabled}";
```

```rust
// crates/trident/src/subsystems/network.rs:56-60
// We need to disable cloud-init's network configuration when
// Trident is configuring the network, otherwise cloud-init may
// deploy additional configurations that are undesired
disable_cloud_init_networking(CLOUD_INIT_CONFIG_DIR)?;
```

**Verdict:** Correctly optional — cloud-init may or may not be present.

---

## 📊 Summary Table

| Module | Type | Permission | Classification | Optional? | Source evidence |
|--------|------|-----------|----------------|-----------|----------------|
| cgroup | `cgroup_t` | `filesystem getattr` | 🟢 DIRECT | No | `FindMnt::run()` in diagnostics.rs:195, newroot.rs:485, selinux.rs:261 |
| cgroup | `cgroup_t` | `create_files_pattern` | 🔴 UNSUBSTANTIATED | **Yes** | No code creates cgroup files |
| cgroup | `memory_pressure_t` | `read open getattr setattr` | 🔴 UNSUBSTANTIATED | **Yes** | Zero matches for `memory_pressure` in source |
| cgroup | `cgroup_t` (udevadm_t) | `filesystem getattr` + read_lnk | 🟢 DIRECT | No | `Dependency::Udevadm` in encryption.rs:402, mkfs.rs:235 |
| gpg | `gpg_agent_exec_t` | `domain_entry_file` | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Gpg*` in enum; architecturally suspect |
| gpg | `gpg_exec_t` | `domain_entry_file` (via gpg_entry_type) | 🔴 UNSUBSTANTIATED | **Yes** | Same |
| gpg | `gpg_secret_t` | `list_dirs_pattern` (via gpg_list_user_secrets) | 🔴 UNSUBSTANTIATED | **Yes** | Same |
| gpg | `gpg_pinentry_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks pinentry; no trident code path |
| gpg | `gpg_secret_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module reads `~/.gnupg/`; no trident code path |
| sudo | `sudo_exec_t` | `can_exec` (full execute) | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Sudo`; trident is always root (lib.rs:230) |
| ntp | `ntpd_exec_t` | `execute` | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Ntpd`; NTP replaced by chrony |
| ntp | `ntpd_exec_t` | `getattr` | 🔵 INDIRECT (dracut) | **Yes** | dracut `01systemd-timesyncd` checks `systemd-timesyncd` (labeled `ntpd_exec_t`) |
| ntp | `ntpd_unit_t` | `getattr` | 🔵 INDIRECT (dracut) | **Yes** | dracut `01systemd-timesyncd` `install()` stats time-related unit files |
| chronyd | `chronyd_exec_t` | `can_exec` (via chronyd_exec) | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Chronyd` in enum; no call site |
| chronyd | config/keys | read (via macros) | 🔴 UNSUBSTANTIATED | **Yes** | No code reads chrony config |
| chronyd | `chronyc_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks `chronyc`; no trident code path |
| chronyd | `chronyd_unit_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks chronyd units; no trident code path |
| chronyd | `chronyd_var_lib_t` | `getattr open read relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/lib/chrony/` (mkinitrd.rs:74-89) |
| chronyd | `chronyd_var_log_t` | `getattr open read relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/log/chrony/` (mkinitrd.rs:74-89) |
| dhcp | `dhcpc_exec_t` | `getattr` | 🔵 INDIRECT (dracut) | **Yes** | dracut `35network-legacy` checks `dhclient` (labeled `dhcpc_exec_t`) |
| dhcp | `dhcpc_state_t` | `getattr open read relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/lib/dhclient/` (mkinitrd.rs:74-89) |
| dhcp | `dhcpd_unit_t` | `getattr` | 🔴 UNSUBSTANTIATED | Already optional | No dracut module checks dhcpd units; no trident code path |
| dhcp | `dhcp_etc_t` | read (via sysnet_read_dhcp_config) | 🟢 DIRECT | No | `Dependency::Netplan` in network.rs:54, netplan.rs:43 |
| kerberos | `krb5kdc_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks `krb5kdc`; no trident code path |
| kerberos | `kadmind_exec_t` | `can_exec` (via kerberos_exec_kadmind) | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Kadmind` in enum |
| kerberos | `krb5_conf_t` | read (via kerberos_read_config) | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module reads krb5.conf; no trident code path |
| logrotate | `logrotate_unit_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks logrotate units; no trident code path |
| logrotate | `logrotate_var_lib_t` | `getattr open read relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/lib/logrotate/` (mkinitrd.rs:74-89) |
| oddjob | `oddjob_mkhomedir_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks `mkhomedir`; no trident code path |
| slocate | `locate_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks `locate`; no trident code path |
| uuidd | `uuidd_exec_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks `uuidd`; no trident code path |
| uuidd | `uuidd_var_lib_t` | `relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/lib/uuidd/` (mkinitrd.rs:74-89) |
| uuidd | `uuidd_var_lib_t` | manage (via uuidd_manage_lib_dirs) | 🔴 UNSUBSTANTIATED | **Yes** | No code references uuidd |
| rpm | `rpm_unit_t` | `getattr` | 🔴 UNSUBSTANTIATED | **Yes** | No dracut module checks RPM units; no trident code path |
| rpm | `rpm_var_lib_t` | `relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/lib/rpm/` (mkinitrd.rs:74-89) |
| rpm | `rpm_var_lib_t` | `add_name create setattr write` | 🔴 UNSUBSTANTIATED | **Yes** | No `Dependency::Rpm` in enum |
| rpm | `rpm_var_cache_t` | `relabelto` | 🔵 INDIRECT (dracut) | **Yes** | dracut `cp -a` on `/var/cache/rpm/` (mkinitrd.rs:74-89) |
| rpm | `rpm_t` | `unlabeled_t` access | ⚪ RPM INSTALL (not trident) | No | trident.spec:%post — actor is package manager |
| rpm | `rpm_script_t` | `security_t`, `unlabeled_t`, `kernel_t` | ⚪ RPM INSTALL (not trident) | No | trident.spec:%post selinux module install |
| cloudinit | all | various | mixed | Already optional | network.rs:14-16, 56-60 |

---

## 📋 Recommendations

### 🔴 UNSUBSTANTIATED — strong candidates for removal

These have zero code backing in the trident repository or dracut. They were
likely added via `audit2allow` during development:

| # | Rule | Why it's suspect |
|---|------|-----------------|
| 1 | `can_exec(trident_t, sudo_exec_t)` | Trident is always root (lib.rs:230); no `Dependency::Sudo` |
| 2 | `domain_entry_file(trident_t, gpg_agent_exec_t)` | Makes gpg-agent an entry point for trident_t — no architectural reason |
| 3 | `gpg_entry_type(trident_t)` | Makes gpg binary an entry point — no GPG usage |
| 4 | `gpg_list_user_secrets(trident_t)` | No code lists ~/.gnupg/ |
| 5 | `gpg_pinentry_exec_t:file getattr` | No dracut module checks pinentry; no trident code path |
| 6 | `gpg_secret_t:file getattr` | No dracut module reads ~/.gnupg/; no trident code path |
| 7 | `kerberos_exec_kadmind(trident_t)` | No `Dependency::Kadmind`; no Kerberos tooling |
| 8 | `kerberos_read_config(trident_t)` | No dracut module reads krb5.conf; no trident code path |
| 9 | `krb5kdc_exec_t:file getattr` | No dracut module checks krb5kdc; no trident code path |
| 10 | `chronyd_exec(trident_t)` | No `Dependency::Chronyd`; no call site |
| 11 | `chronyd_read_config(trident_t)` | No code reads chrony config |
| 12 | `chronyd_read_key_files(trident_t)` | No code reads chrony keys |
| 13 | `chronyc_exec_t:file getattr` | No dracut module checks chronyc; no trident code path |
| 14 | `chronyd_unit_t:file getattr` | No dracut module checks chronyd units; no trident code path |
| 15 | `ntpd_exec_t:file { execute }` | No `Dependency::Ntpd`; NTP replaced by chrony |
| 16 | `create_files_pattern(trident_t, cgroup_t, cgroup_t)` | No cgroup file creation |
| 17 | `fs_watch_memory_pressure(trident_t)` + explicit rule | Zero PSI references in source |
| 18 | `uuidd_manage_lib_dirs(trident_t)` | No code references uuidd |
| 19 | `uuidd_exec_t:file getattr` | No dracut module checks uuidd; no trident code path |
| 20 | `rpm_var_lib_t:file { add_name create setattr write }` | No `Dependency::Rpm`; no RPM DB writes |
| 21 | `rpm_unit_t:file getattr` | No dracut module checks RPM units; no trident code path |
| 22 | `logrotate_unit_t:file getattr` | No dracut module checks logrotate units; no trident code path |
| 23 | `oddjob_mkhomedir_exec_t:file getattr` | No dracut module checks mkhomedir; no trident code path |
| 24 | `locate_exec_t:file getattr` | No dracut module checks locate; no trident code path |
| 25 | `dhcpd_unit_t:file getattr` | No dracut module checks dhcpd units (already optional) |

### 🔵 INDIRECT (dracut) — wrap in `optional_policy`

These permissions exist because dracut runs as `trident_t`. The `relabelto`
comes from `cp -a` restoring SELinux xattrs. The `getattr` comes from dracut
module `check()` → `require_binaries` → `find_binary` → `stat()`. They should
be `optional_policy` so the policy compiles even if the target module is removed:

| # | Rule | Dracut trigger |
|---|------|---------------|
| 1 | `chronyd_var_lib_t:dir { getattr open read relabelto }` | `cp -a` on `/var/lib/chrony/` |
| 2 | `chronyd_var_log_t:dir { getattr open read relabelto }` | `cp -a` on `/var/log/chrony/` |
| 3 | `dhcpc_exec_t:file getattr` | `35network-legacy` checks `dhclient` |
| 4 | `dhcpc_state_t:dir { getattr open read relabelto }` | `cp -a` on `/var/lib/dhclient/` |
| 5 | `ntpd_exec_t:file getattr` | `01systemd-timesyncd` checks `systemd-timesyncd` (labeled `ntpd_exec_t`) |
| 6 | `ntpd_unit_t:file getattr` | `01systemd-timesyncd` `install()` stats time unit files |
| 7 | `logrotate_var_lib_t:dir { getattr open read relabelto }` | `cp -a` on `/var/lib/logrotate/` |
| 8 | `rpm_var_lib_t:dir relabelto` + `file relabelto` | `cp -a` on `/var/lib/rpm/` |
| 9 | `rpm_var_cache_t:dir relabelto` + `file relabelto` | `cp -a` on `/var/cache/rpm/` |
| 10 | `uuidd_var_lib_t:dir relabelto` | `cp -a` on `/var/lib/uuidd/` |

### 🟢 Must remain as hard requirements

| Rule | Justification |
|------|--------------|
| `cgroup_t:filesystem getattr` | `FindMnt::run()` — diagnostics, selinux relabel, newroot |
| `udevadm_t` cgroup rules | `Dependency::Udevadm` — disk encryption, mkfs |
| `sysnet_read_dhcp_config` | `Dependency::Netplan` — network subsystem |

### ⚪ Required for RPM lifecycle (not trident runtime)

| Rule | Justification |
|------|--------------|
| `rpm_t` / `rpm_script_t` rules | trident.spec `%post selinux` — actor is the package manager, not trident |

---

## 📎 Appendix: The Dracut Chain

Trident regenerates the initramfs by calling dracut directly:

```rust
// crates/osutils/src/mkinitrd.rs:47-55
pub fn execute(debug: bool) -> Result<(), TridentError> {
    if Path::new("/usr/bin/mkinitrd").exists() {
        Dependency::Mkinitrd.cmd().run_and_check()
            .structured(ServicingError::RegenerateInitrd)
    } else {
        run_dracut(debug).structured(ServicingError::RegenerateInitrd)
    }
}
```

```rust
// crates/osutils/src/mkinitrd.rs:59-89
fn run_dracut(debug: bool) -> Result<(), Error> {
    let mut script = NamedTempFile::new()?;
    script.write(VERITY_RACE_CONDITION_WORKAROUND.as_bytes())?;
    script.flush()?;
    std::fs::set_permissions(script.path(), std::fs::Permissions::from_mode(0o755))?;

    let mut cmd = Dependency::Dracut.cmd().with_arg("--force");
    cmd.arg("--regenerate-all")
       .arg("--zstd")
       .arg("--include").arg("/usr/lib/locale").arg("/usr/lib/locale")
       .arg("--include").arg(script.path())
       .arg("/lib/dracut/hooks/cmdline/10-verity-workaround.sh")
       .run_and_check()
}
```

`Dependency::Dracut` is in the enum at `dependencies.rs:95`.

**Why this triggers relabel permissions:**
- Dracut runs as `trident_t` — no `dracut_t` type exists in SELinux refpolicy
- Dracut's shell modules use `cp -a` to copy system files into initramfs staging
- `cp -a` preserves SELinux xattrs (`security.selinux`)
- Copying a file with `dhcpc_state_t` label from a `tmp_t` staging dir triggers:
  - `relabelfrom` on the source type (`tmp_t`)
  - `relabelto` on the target type (`dhcpc_state_t`)
- These operations are attributed to `trident_t` because that's the process context

**Why this also triggers `getattr` permissions:**

Dracut iterates over ALL installed modules in `/usr/lib/dracut/modules.d/` and
calls each module's `check()` function to determine whether to include it:

```bash
# /usr/bin/dracut:1812
for_each_module_dir check_module
```

Each `check()` typically calls `require_binaries` to test if required binaries exist:

```bash
# /usr/lib/dracut/dracut-init.sh:107-124
require_binaries() {
    for cmd in "$@"; do
        if ! find_binary "$cmd" &> /dev/null; then
            ((_ret++))
        fi
    done
    return "$_ret"
}
```

`find_binary` resolves to `test -x` / `-L` checks on system paths:

```bash
# /usr/lib/dracut/dracut-functions.sh:49-85
find_binary() {
    # ...
    for p in $DRACUT_PATH; do
        _path="${p}${_delim}${1}"
        if [[ -L ${dracutsysrootdir}${_path} ]] || [[ -x ${dracutsysrootdir}${_path} ]]; then
            printf "%s\n" "${_path}"
            return 0
        fi
    done
    # fallback
    type -P "${1##*/}"
}
```

These `test -x` and `-L` checks perform `stat()` syscalls. Since dracut runs as
`trident_t`, the `stat()` on a file labeled (e.g.) `dhcpc_exec_t` triggers a
`getattr` AVC denial against `trident_t`.

**Confirmed dracut module → SELinux type mappings:**

| Dracut module | Binary checked | SELinux type |
|---------------|---------------|-------------|
| `35network-legacy` | `dhclient` | `dhcpc_exec_t` |
| `91crypt-gpg` | `gpg`, `gpg-agent` | `gpg_exec_t`, `gpg_agent_exec_t` |
| `01systemd-timesyncd` | `systemd-timesyncd` | `ntpd_exec_t` (per `file_contexts`) |
| `01systemd-timedated` | `systemd-timedated` | `ntpd_exec_t` (per `file_contexts`) |

## 📎 Appendix: The Setfiles / Relabel Chain

Trident relabels filesystems after OS modifications:

```rust
// crates/trident/src/subsystems/selinux.rs:234-249
fn perform_relabel(ctx: &EngineContext) -> Result<(), TridentError> {
    let selinux_type = get_selinux_type(SELINUX_CONFIG)?;
    Dependency::Setfiles.cmd()
        .arg("-m")
        .arg(Path::new("/etc/selinux").join(selinux_type)
             .join("contexts/files/file_contexts"))
        .args(filesystems_to_relabel(ctx)?)
        .run_and_check()
}
```

`Dependency::Setfiles` is in the enum at `dependencies.rs:116`.

`setfiles` transitions to `setfiles_t` domain via `seutil_run_setfiles`, so
the actual relabel syscalls run under `setfiles_t`, NOT `trident_t`. This
means the relabel rules on `trident_t` are **not** for setfiles — they're
for dracut.

## 📎 Appendix: Dependency Enum (complete list)

All external tools trident can invoke (`crates/osutils/src/dependencies.rs:91-142`):

```
Blkid, Cryptsetup, Dd, Df, Dracut, Efivar, Efibootmgr, Eject, Findmnt,
Iptables, Journalctl, Losetup, Lsblk, Lsof, Mdadm, Mkdir, Mkfs, Mkinitrd,
Mkswap, Mount, Mountpoint, Netplan, Partx, Setfiles, Sfdisk, Swapoff, Swapon,
Systemctl, SystemdConfext, SystemdCryptenroll, SystemdFirstboot, SystemdPcrlock,
SystemdRepart, SystemdSysext, Touch, Udevadm, Umount, Uname, Veritysetup, Wipefs
```

**Notable absences:** No `Sudo`, `Ntpd`, `Chronyd`, `Kadmind`, `Gpg`,
`GpgAgent`, `Rpm`, `Tdnf`, `Uuidd`.
