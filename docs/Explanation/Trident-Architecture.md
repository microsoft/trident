
# Trident Architecture

<!--
DELETE ME AFTER COMPLETING THE DOCUMENT!
---
Task: https://dev.azure.com/mariner-org/polar/_workitems/edit/13172
Title: Trident Architecture
Type: Explanation
Objective:

Explain in mid detail the architecture of Trident, how it works, what components
it has.
-->

```mermaid
flowchart TD
    user([User])
    Image@{ shape: doc, label: "OS Image (COSI)" }
    HC@{ shape: doc, label: "Host Configuration YAML (HC)" }

    user -- Provides --> Image
    user -- Writes --> HC
    user -- Invokes --> CLI
    CLI -- Reads --> DS
    CORE -- Writes --> DS

    subgraph Trident
        CLI[CLI]
        StaticValidator[Static Validator]
        CORE[Core]
        OSImageReader[OS Image Reader]
        HostConfigReader[Host Config Reader]
        logging[Logging]

        CLI --Starts--> CORE
        CORE <--Retrieve Image--> OSImageReader
        CORE <--Retrieve HC--> HostConfigReader
        CORE --Configures--> logging
        CORE --Starts--> Update
        CORE --Starts--> Install
        CORE --Runs--> Raid
        HostConfigReader <--Validate HC--> StaticValidator
        
        subgraph Engine
            Update[Update]
            Context[Engine Context]
            Install[Install]
            StorageEngine[Storage Provisioning Engine]
            SubsystemTrait[Subsystem Trait]
            Raid[RAID Rebuilder]

            Update --Populates--> Context
            Install --Populates--> Context
            Install --Configures--> StorageEngine
            Raid --Configures--> StorageEngine
        end

        subgraph Subsystems
            direction RL
            MOSConfig[Management OS Configuration]
            ESPConfig[ESP Configuration]
            StorageConfig[Storage Configuration]
            BootConfig[Boot Configuration]
            NetworkConfig[Network Configuration]
            OsConfig[OS Configuration ]
            ManagementConfig[Agent Configuration]
            Hooks[Hooks]
            InitrdConfig[Initrd Configuration]
            SELinuxConfig[SELinux Configuration]
        end

        SubsystemTrait --Implemented by-->Subsystems
        Update --Runs--> Subsystems
        Install --Runs--> Subsystems
        Context --Informs--> Subsystems
        
        OSImageReader --Provides Image--> Context

        Subsystems<--Call--> OSUtils[OS Utils]
    end

    HostConfigReader -- Reads --> HC
    OSImageReader -- Reads --> Image

    subgraph Host
        Binaries
        Storage@{ shape: lin-cyl, label: "Disk storage" }
        DS@{ shape: doc, label: "Persistent Data Store (DS)" }
    end


    OSUtils <--Runs--> Binaries
    OSUtils --Modifies--> Storage
    StorageEngine --Provisions--> Storage
```
