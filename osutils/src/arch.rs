/// System architecture
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemArchitecture {
    /// 32-bit x86
    X86,

    /// 64-bit x86
    Amd64,

    /// 32-bit ARM
    Arm,

    /// 64-bit ARM
    Aarch64,

    /// Other
    Other,
}

impl SystemArchitecture {
    /// Get the current system architecture
    pub fn current() -> Self {
        if cfg!(target_arch = "x86") {
            SystemArchitecture::X86
        } else if cfg!(target_arch = "x86_64") {
            SystemArchitecture::Amd64
        } else if cfg!(target_arch = "arm") {
            SystemArchitecture::Arm
        } else if cfg!(target_arch = "aarch64") {
            SystemArchitecture::Aarch64
        } else {
            SystemArchitecture::Other
        }
    }
}
