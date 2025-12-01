# prevent dracut's mktemp call from creating a path that matches '*.ko*'
# TODO: remove when this is patched in AZL3.0
sed -i 's|dracut.XXXXXX|dracut.dXXXXXX|' /usr/bin/dracut
