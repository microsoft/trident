FROM mcr.microsoft.com/cbl-mariner/base/rust:1

RUN tdnf install -y rpmdevtools openssl-devel clang-devel protobuf-devel

WORKDIR /work

COPY trident.spec .
COPY systemd ./systemd
COPY bin/trident ./target/release/trident
COPY artifacts/osmodifier /usr/src/mariner/SOURCES/osmodifier

ARG TRIDENT_VERSION=dev-build
ARG RPM_VER=0.1.0
ARG RPM_REL=1

RUN \
    sed -i "s/cargo build/#cargo build/g" trident.spec && \
    rpmbuild -bb --build-in-place trident.spec \
    --define="trident_version $TRIDENT_VERSION" \
    --define="rpm_ver $RPM_VER" \
    --define="rpm_rel $RPM_REL" && \
    tar -czvf trident-rpms.tar.gz -C /usr/src/mariner ./RPMS
