# Merge directives reference

In-file `fragments:` apply after the base in declaration order. Local `by-<axis>/<value>.yaml` fragments apply afterward in **matrix axis-declaration order**; the axis declared later has later precedence. This is not alphabetical and not directory-name order.

## Rules

| Data shape | Default merge |
| --- | --- |
| Maps | Deep-merge. |
| Lists | Append. |
| Scalars | Same value is OK; different values conflict unless the later value uses `$set`. |

A later `$set` wins over an earlier `$set`. A plain reassignment after a `$set` still conflicts.

## Directives

| Directive | Valid position | Value | Purpose |
| --- | --- | --- | --- |
| `$set` | Any field value, commonly scalars or whole tailor values | any YAML value | Explicitly override an earlier value. |
| `$replace` | List field value (exclusive) | list | Replace the inherited list wholesale. |
| `$remove` | List field value | list | Remove matching items from the inherited list. |
| `$prepend` | List field value | list | Insert items **before** the inherited list. |
| `$append` | List field value | list | Insert items **after** the inherited list. |
| `$unset` | Mapping value | the bare token `$unset` | Remove the inherited key entirely. |
| `$include` | Mapping value or list item | path string | Splice a shared YAML file at that position. |
| `$select` | — | — | **Reserved, not implemented** — use [`by-<axis>/<value>.yaml` fragments](image-yaml.md#fragments) instead. |

`$prepend`, `$append`, and `$remove` may share one mapping (e.g. trim the inherited list and add to both
ends at once); `$set` and `$replace` are exclusive and cannot be combined with the others.

## `$set`

```yaml
config:
  os:
    hostname:
      $set: gizmo-edge
```

For a whole base override:

```yaml
base:
  $set:
    oci:
      uri: "registry.example/gizmo/base:edge"
      platform: "linux/${arch}"
```

## `$replace`

```yaml
outputs:
  $replace:
    - format: raw
```

## `$remove`

```yaml
config:
  os:
    packages:
      install:
        $remove:
          - base-extra
```

## `$prepend` / `$append`

Lists append by default. `$prepend` puts items at the front; `$append` is the explicit, combinable form of
the default. Use both to wrap an inherited list in one fragment (a plain `$prepend` mapping would otherwise
leave no room for an inline append):

```yaml
config:
  scripts:
    postCustomization:
      $prepend:
        - path: scripts/setup.sh
      $append:
        - path: scripts/teardown.sh
```

For a fragment that prepends to an inherited list `[a, b]`, the result is `[setup.sh, a, b, teardown.sh]`.
Across fragments, a later fragment's `$prepend` lands further toward the front. `$remove` may be combined
too: it drops matching inherited items before the ends are added.

## `$unset`

Set a key's value to the bare token `$unset` to remove that key, so the rendered config omits it:

```yaml
config:
  os:
    selinux: $unset      # drop the inherited selinux block entirely
```

Remove several keys by annotating each; remove a whole subtree by unsetting it at its own level
(`scripts: $unset`). `$unset` is resolved at merge time before interpolation, so a field can never hold the
literal string `$unset` (as with YAML's reserved `null`/`~`). A later fragment may set a removed key again.
The mapping synonym `key: { $unset: true }` is tolerated; `{ $unset: false }` is an error.

## `$include`

```yaml
config:
  storage:
    $include: layouts/storage/pro.yaml
```

The include path is relative to the image directory in user-facing examples. The included file is substituted as the value at that position. If `$include` is a list item and the included file is a list, the elements are spliced into the surrounding list.

`$include` must be the sole key in its mapping. To tweak included content, layer a later fragment with normal merge rules or directives.

Select a subtree with a JSON pointer suffix, so one library can hold several shared values:

```yaml
config:
  scripts:
    postCustomization:
      - interpreter: /bin/bash
        content:
          $include: ../common/scripts.yaml#/audit-directory
```

Pointers use `/`-separated keys or sequence indices; `~1` escapes `/` and `~0`
escapes `~`. A trailing `#` selects the document root. Missing nodes, invalid
escapes and noncanonical sequence indices fail. Only the selected subtree is
expanded, so unrelated library entries need not resolve. Different nodes in the
same library may include each other; repeated selected-node cycles are rejected.

Selected contents are part of the rendered-config fingerprint. To invalidate on
any edit to the containing library, declare its physical path in `extraDependencies`
(for example `../common/scripts.yaml`), never a path containing `#/...`.
