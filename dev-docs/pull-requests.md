# Pull Request Guidelines

This document describes the process for submitting pull requests (PRs) to the
`main` branch of `trident` & related repositories.

- [Pull Request Guidelines](#pull-request-guidelines)
  - [Opening a PR](#opening-a-pr)
    - [Size and Scope](#size-and-scope)
    - [Checks](#checks)
    - [On Drafts](#on-drafts)
    - [Titles](#titles)
    - [Descriptions](#descriptions)
    - [Testing](#testing)
    - [PR Labels](#pr-labels)
  - [Abandoning PRs and Cleaning Up](#abandoning-prs-and-cleaning-up)

## Opening a PR

A pull request generally[[1]](#pr-labels) implies intent to merge code into the
`main` branch.

### Size and Scope

Try to keep PRs small and focused on a single issue. If you have multiple changes
that are not related, submit them as separate PRs.

**In Trident, try to keep PRs focused on a single crate.** If you have changes that
span multiple crates, submit them as separate PRs in order to make reviewing
easier. Of course, **the exception is to ensure that the build works**, sometimes
there is no way around changing things in several places.

For example, if you are adding a new feature to `trident` that requires API
changes, submit the API changes as a separate PR from the feature
implementation.

### Checks

When making a PR you are attempting to merge, make sure that:

- Code is formatted correctly. (`make format`) (See: [Python](./python.md))
- Clippy is happy. (`make check`)
- The build works. (`make check`)
- API docs are up to date. (`make build-api-docs`)
- Unit tests pass. (`make test`)
- Functional tests pass. (`make functional-test`)
- Local make targets work: **(THESE ARE NOT RUN BY CI, CHECK MANUALLY)**
  - `make rpm`
  - `make docker-build`
- Coverage is above the baseline. (`make coverage`)

*Note: Running `make all` will perform these checks.*

### On Drafts

What does it mean for a PR to be a draft?

- **Published** means that you consider the PR to be *feature-complete* and only
  *minor updates* to address feedback are expected. Reviewers will assume this is
  your final design and will comment accordingly. **Published PRs should be at
  the front of the review queue!**
- **Draft** means that the status of the PR is *not final*, and you
  are still working on it, reviewers may leave feedback with the understanding
  that the design may change.
  - In general, a draft PR should be something that is coming soon, unless otherwise
    stated with a label.
  - As mentioned below in [PR Labels](#pr-labels), a draft may also be marked
    with a label to denote that it is *not expected to be merged*.

> NOTE: If a it turns out that a published PR will need significant changes
> after a review, it should be marked as a draft until it is ready to be
> reviewed again. This will let reviewers know that the design will be changed.

### Titles

> NOTE: In the future, we may want to automatically produce changelogs from PR
> titles. To support this effort and have consistent titles, please follow these
> guidelines.

Titles *should* be in the form: `title = [optional label]<type>: <short description>`

Where:

- `<type>` is one of the following:
  - Strictly user-facing changes:
    - `feature`: For creating or updating **user-facing** features.
    - `bug`: For **user-facing** bug fixes.
  - Documentation changes:
    - `docs`: For documentation only & doc generation changes.
  - Otherwise:
    - `engineering`: For changes to our CI, pipelines, publishing, build system,
      makefile, dockerfile, specfile, stable refactors, adding/modifying tests,
      and changes to dependencies. Basically anything that customers don't see nor
      directly care about.
  - See [ECF Dev
    Guide](https://dev.azure.com/mariner-org/ECF/_git/ecf-docs?version=GBmain&path=/TeamDocs/dev-guide.md&_a=preview)
    for more info.
- Optional labels are covered in [PR Labels](#pr-labels). *These are generally
  only used in drafts!*

Examples:

- `feature: Add support for foo`
- `bug: Fix bar`
- `docs: Update README`
- `engineering: Add CI for baz`
- `engineering: Update dependencies`
- `[TEST] engineering: Testing CI stuff` (marked as DRAFT!)
- `[RFC] feature: Add support for qux` (marked as DRAFT!)

### Descriptions

The description of a PR should contain a statement of what is being done, and
why it is being done.

### Testing

A PR is expected to contain unit tests for the added functionality[1].

A PR *should* contain functional tests for the added functionality[1], but they
may be added in a follow up PR for brevity as long as a task for it is linked.

[1] Unless there is agreement against it. E.g. internal tooling, or cases where
it's not worth the effort.

### PR Labels

> **NOTES:**
>
> - Devops has no built in support for labelling PRs, so these should be
> prefixed to titles. **Always wrap labels in square brackets `[...]`** to make
> them easy to identify and parse.
> - These should generally be **marked as DRAFT**.

Opening PRs is useful for many reasons beyond just immediately merging code. For
example: sharing proposals, requesting feedback, or starting discussions in a
way that is close to the code and allows for comments to be added on specific
things. For these scenarios, we use the following prefix labels to indicate the
intent:

- `[UNBLOCK]`: A PR with a change that unblocks other work or a customer, but is
  not considered a proper fix.
- `[RFC]`: A request for comment on a proposal for a new feature or change to an
  existing feature. This tag means you're **actively looking for feedback**. Think
  of it as a way of communicating: "Do you think this is a good idea?".
- `[PROTOTYPE]`: A PR with work in its early stages that is not expected to be
  merged yet. This tag generally means you're looking for feedback on a
  prototype implementation of a feature or change. Think of it as a way of
  communicating: "Do you think this is a good way to implement this?".
- `[DNM]`: Generic tag for "do not merge". This tag means that the PR is on hold
  for some reason, and should not be merged.
- `[DNR]`: Generic tag for "do not review". This tag means you're looking for
  feedback on a PR that is not ready to be reviewed.
- `[TEST]`: A test PR that should not be reviewed or merged. Should be deleted
  once the test is complete.

## Abandoning PRs and Cleaning Up

If you're no longer working on a PR, please abandon it. This will remove it from
the queue of PRs to review. If a PR has been untouched for a while, it
should be abandoned. It may be re-opened later if work resumes.

Please remove any labels that are no longer relevant.
