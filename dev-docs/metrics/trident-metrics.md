# Working with Trident Metrics

Trident uses the [tracing](https://crates.io/crates/tracing) crate to create and
process metric events during its execution. Trident sets up a tracing subscriber
with a custom processing layer that takes tracing events and structures them as
a metric event. Each metric is a JSON object that is written to a .jsonl file in
the /var/lib/trident folder on the target OS and if applicable, sent to
the tracestream endpoint of netlaunch/netlisten.

## Common Terminology

- **span**: A span represents a single unit of work done in a system. Spans can
  be nested and can have parent-child relationships. Trident uses spans to track
  execution times and related data for a specific operation. 
- **event**: An event is a single point in time that captures a specific moment
  of execution or value.
- **subscriber**: A subscriber is a component that listens to tracing events and
  processes them in some way. Trident uses a custom subscriber to process
  tracing events as metric events and write them to a file. 
- **layer**: A layer is a component that wraps a subscriber and can modify or
  filter events before they are passed to the subscriber. Trident uses a custom
  layer to structure tracing events as metric events.

## Metric Data Structure

A metric event from Trident is a JSON object that contains the following fields:
- `timestamp`: The timestamp when the metric event was created.
- `metric_name`: The name of the metric event.
- `value`: A JSON object that contains the data for the metric event. This field
  can contain multiple key-value pairs.
- `additional_fields`: A JSON object that contains additional fields that are
  not part of the `value` field but are still relevant to the metric event such
  as the Trident version.
- `platform_info`: A JSON object that contains information about the platform
  where the metric event was created. This field is populated by Trident and
  includes information such as the kernel version, CPU, memory and the product
  UUID for machine identification.

Example metric event:
```json
{
  "timestamp": "2025-01-03T02:32:25.103146873Z",
  "metric_name": "clean_install_provisioning_secs",
  "value": 145.488028814,
  "additional_fields": {
    "trace_id": "1774a767-a73b-4ddc-b010-1fe73ec847a5",
    "trident_version": "0.3.0"
  },
  "platform_info": {
    "asset_id": "742ae07e-d26d-4554-bbca-c6546bd820f5",
    "kernel_version": "6.6.57.1-2.azl3",
    "os_release": "3.0.20241101",
    "total_cpu": 4,
    "total_memory_gib": 6
  }
}
```

## Requirements for Adding a New Metric Event

Metric events are created in two ways, either by using the `tracing` macro on a
function to track it as a span, or by using `tracing::info!` to create a tracing
event.

### Using `tracing::info!` to Create a Metric Event

In order for a metric to be created and processed by the custom Trident layer,
it requires the following:
- `metric_name` field in the event
- `value` field **or** a set of fields each with their own name and value

Example with singular value: 
```rust
    tracing::info!(
        metric_name = "clean_install_provisioning_secs",
        value = start_time.elapsed().as_secs_f64()
    );
```

Example with multiple values:
```rust
    tracing::info!(
        metric_name = "update_start",
        servicing_type = "AbUpdate",
        servicing_state = "Provisioned",
    );
```

When multiple values are provided, the metric event after processing will have a
`value` field that contains the key-value pairs of all the data passed in the
metric event. For the example above, the processed metric event would have
`"value": {"servicing_state": "Provisioned", "servicing_type": "AbUpdate"}`

The rest of the fields in the metric event are populated by the custom Trident
layer. The `additional_fields` and `platform_info` fields can be updated in the
`populate_additional_fields` and `populate_platform_info` functions in the
`src/logging/tracestream.rs` file. These fields are intended to be standardized
across all Trident metric events. For data that is specific to a metric event,
it can be included as multiple fields when creating the event.

### Using the `tracing` Macro on a New Function

If there is a function that needs to be tracked as a span, the `tracing` macro
can be used to easily have a metric event created for the function. By default,
the metric event will have the same name as the function, and the execution time
will be calculated and included in the `value` field of the metric event with
the key `execution_time`. A custom name for the metric and multiple fields can
also be provided to the `tracing` macro to include other data in the metric
event.

Example with default name and execution time:
```rust
#[tracing::instrument]
pub fn create_raid_arrays(host_config: &HostConfig) -> Result<(), Error> {
    // Function logic
}
```

Example with custom name and multiple fields:
```rust
#[tracing::instrument(name = "raid_creation", fields(num_raid_arrays = host_config.storage.raid.software.len()), skip_all)]
pub fn create_raid_arrays(host_config: &HostConfig) -> Result<(), Error> {
    // Function logic
}
```

### Validating New Metric Events

When adding a new metric event, it is important to validate that the event is
being processed and written correctly. When running Trident locally with
`netlaunch`, a `trident-metrics.jsonl` file will be generated in the `trident`
folder. This file can be checked to see if the new metric events show up as
expected. To view the file, run `cat trident-metrics.jsonl | jq .` to pretty
print the metrics.

## Creating a Power BI Visual for a New Trident Metric

To create a new visual such as a KPI card or a chart in the [Trident KPIs
dashboard](https://msit.powerbi.com/groups/29ed1366-c344-4687-9cee-ef260793813e/reports/e5890d54-19ef-462c-8840-26023b903764/a1c9dd83146b8d8c4ebc?experience=power-bi),
follow these steps:

1. Create a new KQL query that retrieves the relevant data for the new metric.
   This can be done in the [Kusto
   Explorer](https://dataexplorer.azure.com/clusters/bmpperformance.eastus/databases/trident)
   by writing a query that filters the metric events by the `metric_name` field
   and any other relevant fields. See other existing KQL queries in the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository under `queries/trident-kpi-queries/powerbi-queries`. For more
   information on writing KQL queries, refer to the [Kusto Query Language (KQL)
   documentation](https://docs.microsoft.com/en-us/azure/data-explorer/kusto/query/).

2. Add the KQL query to the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository under `queries/trident-kpi-queries/powerbi-queries`. Create a new
   file for the query and name it appropriately.

3. Follow the steps in the [Working with Power BI
   Projects](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/4438/Working-with-PowerBI-Projects)
   wiki to update the Trident Power BI report with the new visual.

## Setting up Alerting for a New Trident Metric

To create an alert for a new Trident metric, follow these steps:

1. Create a new KQL query that calculates the alerting threshold for the new
   metric. The threshold should usually be 10% above or below the 95th
   percentile value of a metric up to the last Trident release. Please refer to
   the exisiting queries in the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository under `queries/trident-kpi-queries/alerting-functions` for
   examples.

2. Once the KQL query is created, navigate to the [Kusto
   Explorer](https://dataexplorer.azure.com/clusters/bmpperformance.eastus/databases/trident)
   and paste the query in the query editor. Run the query to create the Kusto
   function that calculates the alerting threshold for the new metric.

3. Edit the `kpi-thresholds-alerting.kql` file in the
   `queries/trident-kpi-queries/alerting-functions` directory in the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository. Add an entry for the new metric threshold that calls the Kusto
   function in the previous step to calculate the alerting threshold. Run the
   updated query in the Kusto Explorer to update the `KpiThresholds()` function.

4. Validate the new metric's alerting threshold shows up by running
   `KpiThresholds()` function in the Kusto Explorer. The new value should be a
   part of the resulting table shown in the editor.

5. Edit the `metric-values-for-build-id.kql` file in the
   `queries/trident-kpi-queries/alerting-functions` directory in the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository. Add an entry for the new metric that retrieves the metric values
   for the latest build ID. Run the updated query in the Kusto Explorer to
   update the `MetricValuesForBuildId()` function.

6. Validate the new metric values show up in the results by running
   `MetricValuesForBuildId()` function in the Kusto Explorer with a pipeline
   build ID as the parameter. The new metric values should be a part of the
   resulting table shown in the editor.