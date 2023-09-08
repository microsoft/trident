Summary:        Agent for bare metal platform
Name:           trident
Version:        0.1.0
Release:        1%{?dist}
Vendor:         Microsoft Corporation
License:        Proprietary
BuildRequires:  openssl-devel
BuildRequires:  rust
BuildRequires:  systemd-units

Requires:       netplan

%description
Agent for bare metal platform

%files
%{_bindir}/%{name}
%dir /etc/%{name}
%{_unitdir}/%{name}.service

%post
%systemd_post %{name}.service

%preun
%systemd_preun %{name}.service

%postun
%systemd_postun_with_restart %{name}.service

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
%{_unitdir}/%{name}-network.service

%post provisioning
%systemd_post %{name}-network.service

%preun provisioning
%systemd_preun %{name}-network.service

%postun provisioning
%systemd_postun_with_restart %{name}-network.service

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

# ------------------------------------------------------------------------------

%build
cargo build --release

%install
install -D -m 755 target/release/%{name} %{buildroot}/%{_bindir}/%{name}

mkdir -p %{buildroot}%{_unitdir}
install -D -m 644 systemd/%{name}.service %{buildroot}%{_unitdir}/%{name}.service
install -D -m 644 systemd/%{name}-network.service %{buildroot}%{_unitdir}/%{name}-network.service

mkdir -p %{buildroot}/etc/%{name}