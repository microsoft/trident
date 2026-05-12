# Test-only SELinux policy module for Trident
#
# This RPM provides additional SELinux permissions needed only in test/CI
# environments. It layers on top of the base trident-selinux module and
# must NOT be installed in production images.
#
# Permissions included:
#   - Steamboat/CI exec transition (ci_unconfined_t -> trident_t)
#   - Interactive unconfined transition (for manual debugging)

%global selinuxtype targeted
%global modulename trident-test

Summary:        Trident test-only SELinux policy
Name:           trident-test-selinux
Version:        1.0.0
Release:        1%{?dist}
License:        MIT
Vendor:         Microsoft Corporation
Group:          Applications/System
Distribution:   Azure Linux
BuildArch:      noarch

Requires:       trident-selinux
Requires:       selinux-policy-%{selinuxtype}
Requires(post): selinux-policy-%{selinuxtype}
BuildRequires:  selinux-policy-devel
%{?selinux_requires}

%description
Test-only SELinux policy module for Trident. Provides CI/interactive
transitions that are not included in the production trident-selinux package.
This package must NOT be installed in production images.

%build
mkdir -p selinux
cp -p packaging/selinux-policy-trident-test/%{modulename}.fc selinux/
cp -p packaging/selinux-policy-trident-test/%{modulename}.if selinux/
cp -p packaging/selinux-policy-trident-test/%{modulename}.te selinux/

make -f %{_datadir}/selinux/devel/Makefile %{modulename}.pp
bzip2 -9 %{modulename}.pp

%install
install -D -m 0644 %{modulename}.pp.bz2 %{buildroot}%{_datadir}/selinux/packages/%{selinuxtype}/%{modulename}.pp.bz2
install -D -p -m 0644 selinux/%{modulename}.if %{buildroot}%{_datadir}/selinux/devel/include/distributed/%{modulename}.if

%files
%{_datadir}/selinux/packages/%{selinuxtype}/%{modulename}.pp.bz2
%{_datadir}/selinux/devel/include/distributed/%{modulename}.if
%ghost %verify(not md5 size mode mtime) %{_sharedstatedir}/selinux/%{selinuxtype}/active/modules/200/%{modulename}

%pre
%selinux_relabel_pre -s %{selinuxtype}

%post
%selinux_modules_install -s %{selinuxtype} %{_datadir}/selinux/packages/%{selinuxtype}/%{modulename}.pp.bz2

%postun
if [ $1 -eq 0 ]; then
    %selinux_modules_uninstall -s %{selinuxtype} %{modulename}
fi

%posttrans
%selinux_relabel_post -s %{selinuxtype}
