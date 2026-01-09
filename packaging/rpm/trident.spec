%global selinuxtype targeted

Summary:        Agent for bare metal platform
Name:           trident
Version:        %{rpm_ver}
Release:        %{rpm_rel}%{?dist}
Vendor:         Microsoft Corporation
License:        Proprietary
Source1:        osmodifier
Source2:        trident.fc
Source3:        trident.if
Source4:        trident.te
BuildRequires:  openssl-devel
BuildRequires:  rust
BuildRequires:  systemd-units

Requires:       e2fsprogs
Requires:       util-linux
Requires:       dosfstools
Requires:       efibootmgr
Requires:       lsof
Requires:       systemd >= 255
Requires:       systemd-udev
Requires:       (%{name}-selinux if selinux-policy-%{selinuxtype})

# Optional dependencies for various optional features

# For network configuration (os.network, managementOs.network)
Suggests:       netplan        
# For RAID support (storage.raid)
Suggests:       mdadm          
# For encryption support (storage.encryption)
Suggests:       tpm2-tools     
Suggests:       cryptsetup
# For integrity support (storage.verity)     
Suggests:       veritysetup
# For mounting NTFS filesystems
Suggests:       ntfs-3g
# For creating NTFS filesystems
Suggests:       ntfsprogs


%description
Agent for bare metal platform

%files
%{_bindir}/%{name}
%dir /etc/%{name}
%{_bindir}/osmodifier
%{_unitdir}/%{name}d.service
%{_unitdir}/%{name}d.socket

%post
%systemd_post %{name}d.socket

%preun
%systemd_preun %{name}d.socket

%postun
%systemd_postun %{name}d.socket

# ------------------------------------------------------------------------------

%package provisioning
Summary:        Trident files for the provisioning OS
Requires:       %{name}

%description provisioning
Trident files for the provisioning OS

%files provisioning
%{_unitdir}/%{name}-network.service

%post provisioning
%systemd_post %{name}-network.service

%preun provisioning
%systemd_preun %{name}-network.service

%postun provisioning
%systemd_postun_with_restart %{name}-network.service

# ------------------------------------------------------------------------------

%package service
Summary:        Trident files for SystemD update and commit services
Requires:       %{name}
Conflicts:      %{name}-install-service

%description service
Trident files for SystemD commit service

%files service
%{_unitdir}/%{name}.service

%post service
%systemd_post %{name}.service

%preun service
%systemd_preun %{name}.service

%postun service
%systemd_postun_with_restart %{name}.service

# ------------------------------------------------------------------------------

%package install-service
Summary:        Trident files for SystemD install service
Requires:       %{name}
Conflicts:      %{name}-service

%description install-service
Trident files for SystemD install service

%files install-service
%{_unitdir}/%{name}-install.service

%post install-service
%systemd_post %{name}-install.service

%preun install-service
%systemd_preun %{name}-install.service

%postun install-service
%systemd_postun_with_restart %{name}-install.service

# ------------------------------------------------------------------------------

%package selinux
Summary:             Trident SELinux policy
BuildArch:           noarch
Requires:            selinux-policy-%{selinuxtype}
Requires(post):      selinux-policy-%{selinuxtype}
BuildRequires:       selinux-policy-devel
%{?selinux_requires}

%description selinux
Custom SELinux policy module

%files selinux
%{_datadir}/selinux/packages/%{selinuxtype}/%{name}.pp.*
%{_datadir}/selinux/devel/include/distributed/%{name}.if
%ghost %verify(not md5 size mode mtime) %{_sharedstatedir}/selinux/%{selinuxtype}/active/modules/200/%{name}

# SELinux contexts are saved so that only affected files can be
# relabeled after the policy module installation
%pre selinux
%selinux_relabel_pre -s %{selinuxtype}

%post selinux
%selinux_modules_install -s %{selinuxtype} %{_datadir}/selinux/packages/%{selinuxtype}/%{name}.pp.bz2

%postun selinux
if [ $1 -eq 0 ]; then
    %selinux_modules_uninstall -s %{selinuxtype} %{name}
fi

%posttrans selinux
%selinux_relabel_post -s %{selinuxtype}

# ------------------------------------------------------------------------------

%package static-pcrlock-files
Summary:        Statically defined .pcrlock files
Requires:       %{name}

%description static-pcrlock-files
Statically defined .pcrlock files for PCR-based encryption. This is a workaround needed because AZL
3.0 fails to provide these files inside the same package as the systemd-pcrlock binary; this should
be removed once the fix is merged in AZL 4.0.

%files static-pcrlock-files
%{_sharedstatedir}/pcrlock.d

# ------------------------------------------------------------------------------

%build
export TRIDENT_VERSION="%{trident_version}"
cargo build --release

mkdir selinux
cp -p %{SOURCE2} selinux/
cp -p %{SOURCE3} selinux/
cp -p %{SOURCE4} selinux/

make -f %{_datadir}/selinux/devel/Makefile %{name}.pp
bzip2 -9 %{name}.pp

%check
test "$(./target/release/trident --version)" = "trident %{trident_version}"

%install
install -D -m 755 %{SOURCE1} %{buildroot}%{_bindir}/osmodifier

install -D -m 755 target/release/%{name} %{buildroot}/%{_bindir}/%{name}
install -D -m 755 target/release/harpoon2-server %{buildroot}/%{_bindir}/harpoon2-server
install -D -m 644 target/release/trident-grpc.service %{buildroot}/%{_unitdir}/trident-grpc.service

# Copy Trident SELinux policy module to /usr/share/selinux/packages
install -D -m 0644 %{name}.pp.bz2 %{buildroot}%{_datadir}/selinux/packages/%{selinuxtype}/%{name}.pp.bz2
install -D -p -m 0644 selinux/%{name}.if %{buildroot}%{_datadir}/selinux/devel/include/distributed/%{name}.if

mkdir -p %{buildroot}%{_unitdir}
# Commit service
install -D -m 644 systemd/%{name}.service %{buildroot}%{_unitdir}/%{name}.service
# Auto-installation service
install -D -m 644 systemd/%{name}-install.service %{buildroot}%{_unitdir}/%{name}-install.service
# Network configuration service for provisioning OS
install -D -m 644 systemd/%{name}-network.service %{buildroot}%{_unitdir}/%{name}-network.service
# Daemon socket and service
install -D -m 644 systemd/%{name}d.socket %{buildroot}%{_unitdir}/%{name}d.socket
install -D -m 644 systemd/%{name}d.service %{buildroot}%{_unitdir}/%{name}d.service

mkdir -p %{buildroot}/etc/%{name}

# Copy statically defined .pcrlock files into /var/lib/pcrlock.d
pcrlockroot="%{buildroot}%{_sharedstatedir}/pcrlock.d"
mkdir -p "$pcrlockroot"
(
  cd %{_sourcedir}/static-pcrlock-files
  find . -type f -print0 | while IFS= read -r -d '' f; do
      mkdir -p "$pcrlockroot/$(dirname "$f")"
      install -m 644 "$f" "$pcrlockroot/$f"
  done
)
