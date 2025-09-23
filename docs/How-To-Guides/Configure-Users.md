
# Configure Users

This guide shows you how to configure users in your Trident's Host Configuration. If you want to create secure user accounts with SSH access and customize their properties you can follow this file.

For complete configuration reference, see the [User API Reference](../Reference/Host-Configuration/API-Reference/User.md).

## Goals

By following this guide, you will:

1. Create users with secure SSH key-only authentication.
2. Assign users to administrative and functional groups.
3. Configure custom user properties (UID, home directory, startup command).

## Prerequisites

1. **Trident Host Configuration file**
   1. Have an existing Host Configuration file or create a new one.
   2. Ensure the Host Configuration file has the basic structure with [os section](../Reference/Host-Configuration/API-Reference/Os.md).
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
    - name: <Desired User Name>
      sshMode: key-only
      sshPublicKeys:
        - <Public SSH Key content>
```

1. Replace `<Desired User Name>` with your desired username.
2. Replace the `<Public SSH Key content>` with your actual public key content (e.g. `ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI... user@hostname`). Notice that more than one key can be associated to one user.

[sshMode](../Reference/Host-Configuration/API-Reference/SshMode.md) controls SSH access: block (default) or key-only. The user will be created with a locked password (no password can be used to login) and SSH key-only authentication.

### Step 2: Configure multiple users

To create multiple users just add multiple users under the user section. Example:

```yaml
os:
  users:
    - name: <User1 Name>
      sshMode: key-only
      sshPublicKeys:
        - <User1 public SSH Key content>
      secondaryGroups:
        - wheel
    - name: <User2 Name>
      sshMode: key-only
      sshPublicKeys:
        - <User2 public SSH Key content>
    - name: <User3 Name>
      sshMode: key-only
      sshPublicKeys:
        - <User3 public SSH Key content>
```

2. Replace with the desired usernames
3. Replace the SSH keys with the actual public keys content of each respective user.

### Step 3: Set custom user properties

#### Add users to administrative groups

1. Add the `secondaryGroups` property to assign users to existing system groups:

```yaml
os:
  users:
    - name: <Desired User Name>
      sshMode: key-only
      sshPublicKeys:
        - <Public SSH Key content>
      secondaryGroups:
        - <Desired group1>
        - <Desired group2>
```

1. Replace the `secondaryGroups` entries with groups that exist on your target system (for example, `wheel`, which typically provides sudo access).
2. Add other groups as needed for your use case.

#### Configure startup command

The `startupCommand` property sets the default command/shell to be first executed when the user logs in. This is equivalent (and replaces) the shell field in `/etc/passwd`. Example:

```yaml
os:
  users:
    - name: <Desired User Name>
      sshMode: key-only
      sshPublicKeys:
        - <Public SSH Key content>
      startupCommand: /bin/bash
```

#### Configure more available user properties

Configure advanced user properties for specific requirements:

```yaml
os:
  users:
    - name: <Desired User Name>
      uid: 1001
      homeDirectory: /opt/custom-user
      primaryGroup: developers
      sshMode: key-only
      sshPublicKeys:
        - <Public SSH Key content>
      secondaryGroups:
        - wheel
```

- **`uid`**: Specific user ID. If not provided, the system will automatically assign a UID.
- **`homeDirectory`**: Custom home directory path.
- **`primaryGroup`**: The primary group (there can be only one and must exist on system).

#### Disable SSH access

To create a user without SSH access:

```yaml
os:
  users:
    - name: local-only-user
      sshMode: block
```

## Password Authentication

Trident has no built-in mechanisms to provision a [user password](../Reference/Host-Configuration/API-Reference/Password.md) in the user section for security reasons.

## Troubleshooting

**User cannot login via SSH**:

- Verify the SSH key is correctly formatted in the `sshPublicKeys` array.
- Ensure `sshMode` is set to `key-only`
- Check that the used private key corresponds to the public key in the Host Configuration file.

**User cannot access required resources**:

- Verify the groups specified in `primaryGroup` and `secondaryGroups` exist on the **target system**.

**Custom startup command fails**:

- Ensure the specified command/shell exists on the target system.
- Verify the path is correct (e.g., `/bin/bash`, `/usr/scripts/startup.sh`)
