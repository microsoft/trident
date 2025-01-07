# Updating Trident KPIs

The Trident KPIs that are represented in the [Trident KPI
Report](https://msit.powerbi.com/groups/29ed1366-c344-4687-9cee-ef260793813e/reports/e5890d54-19ef-462c-8840-26023b903764/a1c9dd83146b8d8c4ebc?experience=power-bi)
have calculation logic that uses the latest release version of Trident. This
allows us to measure Trident's health **since** the last release and compare the
current KPIs against the thresholds based on data up until the latest public
release. Therefore, when a new Trident version is released, there are a couple
steps that need to be taken to update the release versions used in the Trident
KPI calculations for the dashbboard and alerting.

## Updating the Release Version for KPI Calculations

The Kusto queries for the Power BI dashboard and pipeline alerting live in the
[platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
repository under `queries/trident-kpi-queries`. 

When a new Trident version is released, the following steps need to be taken to
update the release versions used in the Trident KPI calculations for the
dashboard and alerting:

1) Clone the
   [platform-telemetry](https://dev.azure.com/mariner-org/ECF/_git/platform-telemetry)
   repository and navigate to the `bmp-kusto-scripts` directory.
2) There is a Python script that automatically updates the release versions in
   the necessary Kusto queries. Run the following command to update the release
   versions in the Kusto queries:
    ```bash
    python3 update_trident_version.py <new_trident_version>
    ```
    For example, if the new Trident release version is `0.3.2024120301`, run the
    following command:
    ```bash
    python3 update_trident_version.py 0.3.2024120301
    ```
3) The script will update the release versions in the Kusto queries for the
   Trident KPIs. Create a PR with the changes and get it reviewed and merged.

4) Once the PR is merged, the Power BI dashboard needs to be synced with the
   latest changes. Go to the [BMP
   Workspace](https://msit.powerbi.com/groups/29ed1366-c344-4687-9cee-ef260793813e/list?experience=power-bi)
   in Power BI. In the top right corner, click the `Source control` button. In
   the `Updates` tab, the Trident KPI report and semantic model will be listed.
   Click the `Sync` button to have the report pull in the version update
   changes.

5) The Kusto function that powers Trident's pipeline alerting needs to be
   updated as well. The script run in step 2 will update the release version in
   the Kusto function. To update the Kusto function:
    - Navigate to the `queries/trident-kpi-queries/alerting-functions`
      directory, and copy the contents of the `kpi-thresholds-alerting.kql`
      file.
    - Go to the [Kusto
      Explorer](https://dataexplorer.azure.com/clusters/bmpperformance.eastus/databases/trident)
      and paste the contents of the `kpi-thresholds-alerting.kql` file in the
      query editor. Run the query to update the Kusto function for the `trident`
      database.

The latest release version should now be used in the Trident KPI calculations
for the dashboard and alerting.