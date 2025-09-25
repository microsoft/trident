%pre
echo "pre script!"
%end

%pre
echo "pre script2!"
%end

%pre-install
echo "pre-install script!"
%end

%post --interpreter=/usr/bin/python3

print("Hello world!")

with open("/tmp/file.log") as f:
    f.write("#test")

%end