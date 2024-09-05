Summary:        Agent for bare metal platform
Name:           trident
Version:        %{rpm_ver}
Release:        %{rpm_rel}%{?dist}
Vendor:         Microsoft Corporation
License:        Proprietary
Source1:        osmodifier
Source2:        trident-selinuxpolicies.cil
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


%description
Agent for bare metal platform

%files
%{_bindir}/%{name}
%dir /etc/%{name}
%{_bindir}/osmodifier
%{_datadir}/selinux/packages/trident-selinuxpolicies.cil

%post
#!/bin/sh
# Apply required selinux policies only if selinux-policy is present
if rpm -q selinux-policy &> /dev/null; then    
    semodule -i %{_datadir}/selinux/packages/trident-selinuxpolicies.cil
fi

%postun
# If selinux-policy is present, remove the trident-selinuxpolicies module
if rpm -q selinux-policy &> /dev/null; then
    semodule -r trident-selinuxpolicies
fi

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
Summary:        Trident files for SystemD service
Requires:       %{name}

%description service
Trident files for SystemD service

%files service
%{_unitdir}/%{name}.service

%post service
%systemd_post %{name}.service

%preun service
%systemd_preun %{name}.service

%postun service
%systemd_postun_with_restart %{name}.service

# ------------------------------------------------------------------------------

%build
export TRIDENT_VERSION="%{trident_version}"
cargo build --release

%check
test "$(./target/release/trident --version)" = "trident %{trident_version}"

%install
install -D -m 755 %{SOURCE1} %{buildroot}%{_bindir}/osmodifier

install -D -m 755 target/release/%{name} %{buildroot}/%{_bindir}/%{name}

mkdir -p %{buildroot}%{_unitdir}
install -D -m 644 systemd/%{name}.service %{buildroot}%{_unitdir}/%{name}.service
install -D -m 644 systemd/%{name}-network.service %{buildroot}%{_unitdir}/%{name}-network.service

mkdir -p %{buildroot}/etc/%{name}

# Copy the trident-selinuxpolicies file to /usr/share/selinux/packages/
mkdir -p %{buildroot}%{_datadir}/selinux/packages/
install -m 755 %{SOURCE2} %{buildroot}%{_datadir}/selinux/packages/