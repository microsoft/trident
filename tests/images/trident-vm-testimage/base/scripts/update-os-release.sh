printf -v DATE '%(%Y%m%d)T' -1
sed -i "s/VERSION=\".*\"/VERSION=\"$DATE\"/" /etc/os-release