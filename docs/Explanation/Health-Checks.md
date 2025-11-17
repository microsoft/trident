
# Health Checks

`Health checks` have been implemented to enable customers to define whether a
servicing operation leaves the target OS in a healthy state. These
`health checks` are optionally run during `trident commit` (the last step of a
clean install or an A/B update). The `health checks` can include
user-defined [scripts](../Reference/Host-Configuration/API-Reference/Script.md)
and/or configurations to verify that
[systemd services are running](../Reference/Host-Configuration/API-Reference/SystemdCheck.md).

If any health check fails:

* **for A/B update**: a rollback will be initiated by `trident commit`,
  updating the Host Status state to `AbUpdateHealthCheckFailed` and triggering
  a reboot into the previous OS. Within the previous OS, `trident commit` will
  validate the boot partition and update the Host Status state to `Provisioned`
  (reflecting that the machine is now Provisioned to the previous OS).
* **for clean install**: a rollback will **NOT** be initiated as there is no
  previous OS. Instead, the Host Status state will be set to `NotProvisioned`.

## Configuring Health Checks

Health checks can be configured in the Host Configuration file under the
[`health.checks`](../Reference/Host-Configuration/API-Reference/Health.md#checks-optional)
section. Any number of [scripts](../Reference/Host-Configuration/API-Reference/Script.md)
and/or [systemd checks](../Reference/Host-Configuration/API-Reference/SystemdCheck.md)
can be defined.

Scripts here are like the other scripts in Trident (e.g.
[preServicing](../Reference/Host-Configuration/API-Reference/Scripts.md#preservicing-optional)),
for example, an inline script can be defined in `health.checks` to query the
network or some Kubernetes state like this:

```yaml
health:
  checks:
  - name: sample-commit-script
    runOn:
    - ab-update
    - clean-install
    content: |
      if ! ping -c 1 8.8.8.8; then
        echo "Network is down"
        exit 1
      fi
      if ! kubectl get nodes; then
        echo "Kubernetes nodes not reachable"
        exit 1
      fi
```

[Systemd checks](../Reference/Host-Configuration/API-Reference/SystemdCheck.md)
can also be defined to ensure that critical systemd services are running after
servicing. For example, to ensure that `kubelet.service` and `docker.service`
are running within 15 seconds of `trident commit` being called for both clean
install and A/B update servicing types:

```yaml
health:
  checks:
  - name: sample-systemd-check
    runOn:
    - ab-update
    - clean-install
    systemdServices:
    - kubelet.service
    - docker.service
    timeoutSeconds: 15
```

## Behavior

Health checks are run during `trident commit` after a `trident install` or
`trident update` have staged and finalized. You can see how `health checks`
fit into the overall servicing flow in these diagrams:

### Clean Install with Health Checks

```mermaid
---
config:
      theme: redux
---
flowchart TD
        A(["Clean Install (to A)"])
        style A color:#085
        A --> B["CleanInstallStaged"]
        B --> C["CleanInstallFinalized<br/>(reboot)"]
        C --> D{"Commit<br/>(unknown OS)"}
        D --booted in A--> XX("in target OS (A)")
        style XX color:#085
        XX --health checks<br/>succeeded--> F["Provisioned (A)<br/>no errors"]
        style F color:#085
        D --did NOT boot in A--> YY("in unepected OS")
        style YY color:#822
        YY --> E["NotProvisioned with last_error set"]
        style E color:#822
        XX --health check<br/>failed--> E
        XX --commit failure--> E
```

### A/B Update with Health Checks

```mermaid
---
config:
      theme: redux
---
flowchart TD
        AA["Provisioned (A)"]
        style AA color:#085
        AA --> A(["A/B Update<br/>from servicing OS A<br/>to target OS B"])
        style A color:#085
        A --> B["AbUpdateStaged"]
        B --> C["AbUpdateFinalized<br/>(reboot)"]
        C --> D{"Commit<br/>(unknown OS)"}
        D --booted in B--> XX("in target OS (B)")
        style XX color:#085
        XX --health checks<br/>succeeded--> F["Provisioned (B)<br/>no errors"]
        style F color:#085
        XX --commit infra failure<br/>last_error set --> Z["AbUpdateRollbackFailed (B)"]
        style Z color:#822
        XX --health checks<br/>failed--> G["AbUpdateHealthCheckFailed"]
        style G color:#822
        D --booted in A--> YY("in servicing OS (A)")
        style YY color:#822
        G --> GG["Auto-rollback<br/>(reboot)"]
        GG --> H{"Commit<br/>(unknown OS)"}
        H --failed to rollback<br/>in target OS (B)--> Z
        H --rolled back<br/>servicing OS (A)--> YY
        YY --> J["Provisioned (A)<br/>with last_error set"]
        style J color:#822
```

## Health Check failures

If a health check fails, the output from each failed check will be captured in
a log file located at
`/var/lib/trident/trident-health-check-failure-<timestamp>.log`
on the target OS. This log file can be used to help diagnose the reason
for the health check failure.

The failures will also be reported in the Trident Host Status `lastError`
field.
