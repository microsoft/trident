# Release Process

The purpose of this document is to document steps to be performed before and
after release.

## Before

- Perform manual validation steps.
- Write release notes by skimming the list of completed PRs since the last
  release and adding entries for each relevent one.

  While drafting the release notes, create a page on the wiki under [Trident
  Releases](https://dev.azure.com/mariner-org/ECF/_wiki/wikis/MarinerHCI.wiki/3306/Trident-Releases)
  but be sure to have `[unreleased]` in the title for now.

  Use the following template:

    ```text
    This release includes many features additions and improvements including...
    (Needed so that email preview text doesn't start with "Breaking Changes")

    # Breaking changes
        *
    # New features
        *
    # Fixes
        *
    # Known issues
        *
    # Links
        * Docs – [README](todo), [API docs](todo)
        * Download – [RPM](todo)
        * Release date - todo
        * Commit – [hash](todo)
        * Experimental RPM that can be installed on Azure Linux 3.0 - [download link](todo) (select trident-binaries3/)

    Subscribe to [Mariner BareMetal Announcements](https://idwebelements.microsoft.com/GroupManagement.aspx?Group=MarinerBMPBroadcast&Operation=join) to hear about future releases. And if you have any issues, please [contact us on Teams](https://teams.microsoft.com/v2/?tenantId=72f988bf-86f1-41af-91ab-2d7cd011db47).
    ```
  
    ADO makes it slightly difficult to get permalinks to files. To do so, you
    first need to navigate to the commit you want a permalink for. Then click
    "Browse files" in the top right and navigate to the specific file you want
    to link to. Once you've opened the file, you can copy the URL and paste it
    into the release notes.

- Ensure documentation for changes highlighed in the release notes is up to date.
- Document known issues and make sure none of them are release blockers.

## On

- Run [the release
  pipeline](https://dev.azure.com/mariner-org/ECF/_build?definitionId=5075) with
  the version from the prerelease feed that will be published. (The pipeline
  will download RPMs from [the prerelease
  feed](https://dev.azure.com/mariner-org/ECF/_artifacts/feed/Trident/UPack/rpms-prerelease/versions/)
  and upload them to the release feed.)
- Create a new branch based on the specific commit being released and call it
 `releases/<name_of_release>`.
- In the Trident wiki publishing page, select the releases/X branch created in
  the last step and publish it.
- After the wiki is published, edit the release notes by copying over the URL of
  the top wiki page to the "API docs" field.
- Update the release notes title to remove the `[unreleased]` tag.
- Copy the release notes into an email, fix the formatting and send to
  [marinerbmpbroadcast@microsoft.com](emailto:marinerbmpbroadcast@microsoft.com).

## After

- Update `cargo.toml` across the crates.
