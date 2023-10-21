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

RUN \
    --mount=type=cache,target=/work/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    rpmbuild -bb --build-in-place trident.spec && \
    tar -czvf trident.tar.gz -C /usr/src/mariner ./RPMS
