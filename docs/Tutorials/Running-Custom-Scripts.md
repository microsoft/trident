
# Running Custom Scripts

In this tutorial, we will learn how to add a custom script to the
[Hello World tutorial](./Trident-Hello-World.md) so that we can execute some
custom logic during the installation.

## Introduction

Trident has several [script hooks](../Explanation/Script-Hooks.md) to allow
users to customize the install and update process. Here, we will create a
postConfiguration script that will run inside the target OS after the system
has been installed and configured, but before the system is rebooted into the
new OS.

## Prerequisites

The prerequisites are the same as for the
[Hello World tutorial](./Trident-Hello-World.md#prerequisites).

## Instructions

### Step 1: Follow Step 1 of the Hello World Tutorial

Follow [Step 1 of the Hello World Tutorial](./Trident-Hello-World.md#step-1-create-the-cosi-file-and-host-configuration)
which will create several files, including the Host Configuration file:
`$HOME/staging/host-config.yaml`

### Step 2: Update the Host Configuration to Use the Script

Edit the Host Configuration file (`$HOME/staging/host-config.yaml`) by
appending the following to the end of the file:

``` yaml
scripts:
  postConfiguration:
    - content: |
        echo "Hello from the post-configuration script!" > /root/post-configuration.log
```

We define the post-configuration script contents inline using the `content`
field for simplicity, but you could also use the `path` field to reference a
script file from the servicing OS filesystem.

### Step 3: Follow Steps 2-6 of the Hello World Tutorial

Follow [Steps 2-6 of the Hello World Tutorial](./Trident-Hello-World.md#step-2-build-a-servicing-iso)
to create the Servicing ISO, boot the target system from the ISO, and install
Azure Linux.

### Step 4: Verify the Script Ran

``` bash
# From your host machine:
ssh tutorial-user@<system-ip-address> sudo cat /root/post-configuration.log
```

You should see the expected message:
`Hello from the post-configuration script!`
