Summary:        Agent for bare metal platform
Name:           trident
Version:        %{rpm_ver}
Release:        %{rpm_rel}%{?dist}
Vendor:         Microsoft Corporation
License:        Proprietary
Source1:        osmodifier
BuildRequires:  openssl-devel
BuildRequires:  rust
BuildRequires:  systemd-units

Requires:       netplan

%description
Agent for bare metal platform

%files
%{_bindir}/%{name}
%dir /etc/%{name}
%{_bindir}/osmodifier

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
