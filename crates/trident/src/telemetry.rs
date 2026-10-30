pub(crate) mod events {
    pub const AVERAGE_MEMORY_USAGE: &str = "average_memory_usage";
    pub const CLEAN_INSTALL_PROVISIONING_SECS: &str = "clean_install_provisioning_secs";
    pub const CLEAN_INSTALL_START: &str = "clean_install_start";
    pub const PEAK_MEMORY_USAGE: &str = "peak_memory_usage";
    pub const STREAM_IMAGE_START: &str = "stream_image_start";
    pub const TOTAL_CPU_TIME: &str = "total_cpu_time";
    pub const TOTAL_NETWORK_USAGE: &str = "total_network_usage";
    pub const TRIDENT_START: &str = "trident_start";
    pub const TRIDENT_SYSTEM_REBOOT: &str = "trident_system_reboot";
    pub const UPDATE_START: &str = "update_start";
    pub const UPDATE_TIME_SECS: &str = "update_time_secs";
}

pub(crate) mod fields {
    pub const ASSET_ID: &str = "asset_id";
    pub const EXECUTION_TIME: &str = "execution_time";
    pub const KERNEL_VERSION: &str = "kernel_version";
    pub const METRIC_NAME: &str = "metric_name";
    pub const OS_RELEASE: &str = "os_release";
    pub const TOTAL_CPU: &str = "total_cpu";
    pub const TOTAL_MEMORY_GIB: &str = "total_memory_gib";
    pub const TRIDENT_VERSION: &str = "trident_version";
    pub const VALUE: &str = "value";
}
