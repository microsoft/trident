# Structured Error in Trident

In Trident, structured error is crucial since it allows to communicate the
failures to the user in a more organized way, as well as to understand within
Trident itself why a certain failure occurred and how it can be resolved.
Structured error in Trident is built around a struct called **`TridentError`**.

In the runtime, `TridentError` will be automatically printed by Trident as
`error!(...)`. Moreover, the Host Status provided to the customer will also
include the contents of `TridentError` under the `lastError` section.

`TridentError` references the inner `ErrorKind`. In turn, `ErrorKind` is an
enum that contains multiple values that describe different categories of
potential errors that might occur on a host and/or in Trident:

1. `ExecutionEnvironmentMisconfiguration` identifies errors that occur when
    the execution environment is misconfigured. E.g. when Trident is run on the
    host without the required root permissions, the error is due to a
    misconfiguration of the execution environment.

2. `Initialization` identifies errors that occur when Trident fails to
    initialize. E.g. when Trident fails to load the local Trident config, the
    entire initialization fails.

3. `Internal` identifies errors that occur due to an internal bug or failure in
    Trident. E.g. failing to serialize or send the Host Status are examples of
    such an internal error.

4. `InvalidInput` identifies errors that occur the user provides invalid input.
    E.g. when the user points Trident to a local Host Configuration file that
    cannot be parsed, it means that the user provided an incorrectly formatted
    YAML file. Defined in a separate file `trident_apt/src/config/host/error.rs`,
    `HostConfigurationStaticValidationError` and `HostConfigurationDynamicValidationError`
    are two key sub-categories that describe potential errors in the Host
    Configuration provided by the user.

5. `Servicing` identifies errors that occur during servicing and require
    user investigation, to determine whether the error occurred due to an
    internal failure in Trident, a failure in one of its dependencies, or a
    system misconfiguration. E.g. `DatastoreError` is within this category
    since loading or writing to a datastore might fail for a variety of reasons.

## Adding a new error type

1. When adding a new error type, e.g. when implementing new functionality or
    modifying a function to return structured error, first determine the
    character of the error. Which category does it belong to?

2. Review the relevant error category, making use of the fact that each one
    is ordered alphabetically, and ensure that such an error type does not
    exist yet. If no category matches, add a new category.

3. Summarize the error in the **error attribute**, or the string message that
    will be displayed when the error variant is triggered. Try to follow one of
    the three patterns:
    - "Failed to do X..."
    - "Y check failed"
    - "W is true, but Z must be true" (This is mostly relevant to the error
    categories related to invalid Host Configuration since you want to
    immediately communicate to the user why a specific input is wrong and how
    it can be fixed.)

4. Create a brief yet descriptive **error type name**. For errors where a
    certain *action* failed, the name could be the action itself: e.g. if
    Trident failed to load the local Trident config, naming the error
    `LoadTridentConfig` is helpful because it's both short and informative.
    There is no need to name it `LoadTridentConfigFailed` or `LoadTridentConfigError`
    since the failure is implied by default. For errors that occurred because
    the user failed to provide the expected input, the name can clarify what
    was done wrong. E.g. `DuplicateUsernames` clearly suggests that the user
    provided duplicate usernames, while the error attribute can additionally
    clarify what the duplicate is and explain that each username has to be
    unique.
