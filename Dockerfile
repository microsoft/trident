FROM mcr.microsoft.com/cbl-mariner/base/rust:1

RUN tdnf install -y rpmdevtools openssl-devel clang-devel protobuf-devel

WORKDIR /work

COPY trident.spec .
COPY systemd ./systemd
COPY Cargo.toml .
COPY Cargo.lock .
COPY proto ./proto
COPY build.rs .
COPY src ./src
COPY trident_api ./trident_api
COPY setsail ./setsail
COPY osutils ./osutils
COPY artifacts/osmodifier /usr/src/mariner/SOURCES/osmodifier
COPY docbuilder ./docbuilder
COPY pytest_gen ./pytest_gen
COPY pytest ./pytest

ARG TRIDENT_VERSION=dev-build
ARG RPM_VER=0.1.0
ARG RPM_REL=1

RUN \
    --mount=type=cache,target=/work/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    rpmbuild -bb --build-in-place trident.spec \
    --define="trident_version $TRIDENT_VERSION" \
    --define="rpm_ver $RPM_VER" \
    --define="rpm_rel $RPM_REL" && \
    tar -czvf trident.tar.gz -C /usr/src/mariner ./RPMS
