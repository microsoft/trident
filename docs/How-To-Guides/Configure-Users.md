
# Configure Users

This guide shows you how to configure users in your Trident Host Configuration. If you want to create a secure user accounts with SSH access and customize their properties you can follow this file.

For complete configuration reference, see the [User API Reference](../Reference/Host-Configuration/API-Reference/User.md).

## Table of Contents

- [Configure Users](#configure-users)
  - [Table of Contents](#table-of-contents)
  - [Goals](#goals)
  - [Prerequisites](#prerequisites)
  - [Instructions](#instructions)
    - [Step 1: Create a basic user with SSH access](#step-1-create-a-basic-user-with-ssh-access)
    - [Step 2: Add users to administrative groups](#step-2-add-users-to-administrative-groups)
    - [Step 3: Configure multiple users](#step-3-configure-multiple-users)
    - [Step 4: Set custom user properties](#step-4-set-custom-user-properties)
      - [Available user properties:](#available-user-properties)
      - [Configure startup command](#configure-startup-command)
      - [Disable SSH access](#disable-ssh-access)
  - [Password Authentication:](#password-authentication)
  - [Troubleshooting](#troubleshooting)

## Goals

By following this guide, you will:

1. Create users with secure SSH key-only authentication.
2. Assign users to administrative and functional groups.
3. Configure custom user properties (UID, home directory, startup command).

## Prerequisites

1. **Trident Host Configuration file**
   1. Have an existing Host Configuration file or create a new one.
   2. Ensure the HC file has the basic structure with [os section](../Reference/Host-Configuration/API-Reference/Os.md).
2. **SSH Public Keys**
   1. Have the SSH key pairs that will be used for ssh authentication.
   2. Obtain the public key content (`.pub` file).
3. **System Groups Knowledge**
   1. Know which groups exist on your target system (e.g., `wheel`, `docker`)

## Instructions

### Step 1: Create a basic user with SSH access

1. Add a `users` section under `os` in your Host Configuration:

```yaml
os:
  users:
    - name: myuser
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
```

2. Replace `myuser` with your desired username.
3. Replace the SSH key with your actual public key content (e.g. `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... user@hostname`). Notice that more than one key can be associated to one user.

The user will be created with a locked password (no password can be used to login) and SSH key-only authentication.

### Step 2: Add users to administrative groups

1. Add the `secondaryGroups` property to assign users to existing system groups:

```yaml
os:
  users:
    - name: admin-user
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
      secondaryGroups:
        - wheel
        - docker
```

2. The `wheel` group typically provides sudo access.
3. Add other groups as needed for your use case.

### Step 3: Configure multiple users

To create multiple users just add multiple users under the user section. Example:

```yaml
os:
  users:
    - name: admin-user
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
      secondaryGroups:
        - wheel
    - name: developer
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
    - name: readonly-user
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
```

2. Replace with the desired usernames
3. Replace the SSH keys with your actual public keys content of each respctive user.

### Step 4: Set custom user properties

Configure advanced user properties for specific requirements:
```yaml
os:
  users:
    - name: custom-user
      uid: 1001
      homeDirectory: /opt/custom-user
      primaryGroup: developers
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
      secondaryGroups:
        - wheel
```

#### Available user properties:
- **`uid`**: Specific user ID. If not provided, the system will automatically assign a UID.
- **`homeDirectory`**: Custom home directory path.
- **`primaryGroup`**: Primary group (must exist on system). 

#### Configure startup command

The `startupCommand` property sets the default command/shell to be first executed when the user logs in. This is equivalent (and replaces) the shell field in `/etc/passwd`. Example:

```yaml
os:
  users:
    - name: service-user
      uid: 1001
      homeDirectory: /var/service-user
      primaryGroup: services
      sshMode: key-only
      sshPublicKeys:
        - <Public Key content>
      startupCommand: /bin/bash
```

#### Disable SSH access

To create a user without SSH access:

```yaml
os:
  users:
    - name: local-only-user
      sshMode: block
```

## Password Authentication: 

Trident has no build mechanisms to provision a [user password](../Reference/Host-Configuration/API-Reference/Password.md) in the user section for security reasons.

## Troubleshooting

**User cannot login via SSH**:
- Verify the SSH key is correctly formatted in the `sshPublicKeys` array.
- Ensure `sshMode` is set to `key-only`
- Check that the used private key corresponds to the public key in the Host Configuration file.

**User cannot access required resources**:
- Verify the groups specified in  in `primaryGroup` and `secondaryGroups` exist on the **target system**.

**Custom startup command fails**:
- Ensure the specified command/shell exists on the target system.
- Verify the path is correct (e.g., `/bin/bash`, `/usr/scripts/startup.sh`)
