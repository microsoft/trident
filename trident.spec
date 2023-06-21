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

%build
cargo build --release

%install
install -D -m 755 target/release/%{name} %{buildroot}/%{_bindir}/%{name}

mkdir -p %{buildroot}%{_unitdir}
install -D -m 644 %{name}.service %{buildroot}%{_unitdir}/%{name}.service

mkdir -p %{buildroot}/etc/%{name}

%post
%systemd_post %{name}.service

%preun
%systemd_preun %{name}.service

%postun
%systemd_postun_with_restart %{name}.service

%files
%{_bindir}/%{name}
%{_unitdir}/%{name}.service
%dir /etc/%{name}
