# Manual Validation

The purpose of this document is to identify steps to be validated manually that
are important and not yet covered by pipelines.

## When

Manual validation should be performed before merging a change. This is to ensure
that you do not randomize your colleagues and that a regression does not get
into the official pipeline, which runs nightly, and produces preview builds.

For PRs, you can skip these steps for any documentation changes or changes that
only affect strings that are not deployment critical. For other PRs, if you
believe they should not affect the deployment, you can ask on the Trident Crew chat.

Manual validation should be also performed as part of final release, which
graduates one of the preview releases into a release.

## What

- [Container validation](validating-container.md)
