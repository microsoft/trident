# Manual Rollback

`Manual rollback` allows users to "undo" recent update(s). There are two types of updates: A/B updates and Runtime updates.

For A/B updates, an update populates the inactive volume with a new target OS and then boots into it. This leaves the previously active OS in the newly inactive volume. Because of this, `Manual rollback` is able to boot into the inactive volume to restore the previously active OS, but is also restricted to only rolling back 1 (the last) A/B update.

For Runtime updates, an update modifies OS components like sysexts and confexts without switching active volumes. Because of this, runtime updates applied to the current active partition can be rolled back one-at-a-time.

## API to Support Rollback

* `trident rollback` is provided to rollback the last update (note that this could be either a Runtime update or an A/B update)
* `trident rollback -ab` can be used to rollback the last A/B update and boot into the inactive OS
* `trident rollback --runtime` can be used to only rollback the last update if it was a runtime update
* `trident rollback --check` can be used to determine what rollback will be invoked (output will be one of: `ab`, `runtime`, or `none`)
* `trident get rollback-chain` can be used to see a yaml list of Host Statuses that can be rolled back to
* `trident get rollback-target` can be used to see the Host Configuration that will be rolled back to
