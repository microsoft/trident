# Merge directives reference

Fragments apply after `image.yaml`. Local `by-<axis>/<value>.yaml` fragments apply in **matrix axis-declaration order**; the axis declared later has later precedence. This is not alphabetical and not directory-name order.

## Rules

| Data shape | Default merge |
| --- | --- |
| Maps | Deep-merge. |
| Lists | Append. |
| Scalars | Same value is OK; different values conflict unless the later value uses `$set`. |

A later `$set` wins over an earlier `$set`. A plain reassignment after a `$set` still conflicts.

## Field-specific merging

Only `base`, `outputs`, `params`, `rpmSources`, `extraParams`, `skip`, `config`, and the
predicate-only `match` are read from `by-*/*.yaml` fragments. `name`, `toolchain`, `toolsDir`,
`matrix`, `selectors`, `features`, `operation`, `outputArtifacts`, `signing`,
`extraDependencies`, `dependsOn`, and `inputs` belong in `image.yaml`; putting them in a
fragment does **not** change the cell.

| Fragment field | Behavior |
| --- | --- |
| `config` | Deep-merge IC mappings; lists append; differing plain scalars conflict. Directives below apply, including `$include` in the `config:` tree. |
| `base` | Merge mappings with the same rules. Exactly one source kind must remain; use `$set` on the whole `base` to switch kinds. |
| `outputs` | Append output specs by default; use `$replace` (or `$set`) on the whole list to replace it. |
| `params` | Repeat the same scalar value freely; different values for one name error. Values are compared as strings, not overridden with `$set`. |
| `rpmSources`, `extraParams` | Concatenate base and matched fragment lists in merge order; no list directives. |
| `skip` | Last specified boolean wins (`skip: false` can undo an earlier `true`). |
| `match` | Filters this fragment together with its path predicate; never merged into the config. |

`$include` resolves **only inside `config:`**, relative to the image directory. It does not
include a `base`, `outputs`, or another top-level tailor field. See
[Fragments and inline match](image-yaml.md#fragments).

## Directives

| Directive | Valid position | Value | Purpose |
| --- | --- | --- | --- |
| `$set` | Any field value, commonly scalars or whole tailor values | any YAML value | Explicitly override an earlier value. |
| `$replace` | List field value (exclusive) | list | Replace the inherited list wholesale. |
| `$remove` | List field value | list | Remove matching items from the inherited list. |
| `$prepend` | List field value | list | Insert items **before** the inherited list. |
| `$append` | List field value | list | Insert items **after** the inherited list. |
| `$unset` | Mapping value | the bare token `$unset` | Remove the inherited key entirely. |
| `$include` | Mapping value or list item **inside `config:`** | path string | Splice a shared YAML file at that position. |
| `$select` | — | — | **Reserved, not implemented** — use [`by-<axis>/<value>.yaml` fragments](image-yaml.md#fragments) instead. |
| `$rename` | — | — | **Unsupported**; rejected by the merger. |

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
literal string `$unset`. A later fragment may set a removed key again.
The mapping synonym `key: { $unset: true }` is tolerated; `{ $unset: false }` is an error.
Unlike `$unset`, YAML `null`/`~` is an ordinary scalar for merge purposes, not a removal directive.

## `$include`

```yaml
config:
  storage:
    $include: layouts/storage/pro.yaml
```

The include path is relative to the image directory in user-facing examples. The included file is substituted as the value at that position. If `$include` is a list item and the included file is a list, the elements are spliced into the surrounding list.

`$include` must be the sole key in its mapping. To tweak included content, layer a later fragment with normal merge rules or directives.
