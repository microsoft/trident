FROM mcr.microsoft.com/cbl-mariner/base/rust:1

RUN tdnf install -y rpmdevtools openssl-devel clang-devel protobuf-devel

WORKDIR /work

COPY trident.spec .
COPY trident.service .
COPY Cargo.toml .
COPY proto ./proto
COPY build.rs .
COPY src ./src

RUN \
    --mount=type=cache,target=/work/target \
    --mount=type=cache,target=/usr/local/cargo/registry \
    rpmbuild -bb --build-in-place trident.spec
