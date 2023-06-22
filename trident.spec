Summary:        Agent for bare metal platform
Name:           trident
Version:        0.1.0
Release:        1%{?dist}
Vendor:         Microsoft Corporation
License:        Proprietary
BuildRequires:  openssl-devel
BuildRequires:  rust
BuildRequires:  systemd-units

%description
Agent for bare metal platform

%files
%{_bindir}/%{name}
%dir /etc/%{name}

# ------------------------------------------------------------------------------

%package provisioning
Summary:        Trident files for the provisioning OS
Requires:       %{name}

# Only one service package is permitted
Provides:       %{name}-service
Conflicts:      %{name}-service

%description provisioning
Trident files for the provisioning OS

%files provisioning
%{_unitdir}/%{name}-provisioning.service

%post provisioning
%systemd_post %{name}-provisioning.service

%preun provisioning
%systemd_preun %{name}-provisioning.service

%postun provisioning
%systemd_postun_with_restart %{name}-provisioning.service

# ------------------------------------------------------------------------------

%package runtime
Summary:        Trident files for the runtime OS
Requires:       %{name}

# Only one service package is permitted
Provides:       %{name}-service
Conflicts:      %{name}-service

%description runtime
Trident files for the runtime OS

%files runtime
%{_unitdir}/%{name}.service

%post runtime
%systemd_post %{name}.service

%preun runtime
%systemd_preun %{name}.service

%postun runtime
%systemd_postun_with_restart %{name}.service

# ------------------------------------------------------------------------------

%build
cargo build --release

%install
install -D -m 755 target/release/%{name} %{buildroot}/%{_bindir}/%{name}

mkdir -p %{buildroot}%{_unitdir}
install -D -m 644 systemd/%{name}.service %{buildroot}%{_unitdir}/%{name}.service
install -D -m 644 systemd/%{name}-provisioning.service %{buildroot}%{_unitdir}/%{name}-provisioning.service

mkdir -p %{buildroot}/etc/%{name}