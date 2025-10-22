# Trident Public Documentation Standards

Standards and conventions for writing public documentation for Trident.

## General Structure

Trident documentation lives in the `/docs` directory. The documentation is
organized following the *[Diátaxis](https://diataxis.fr/)* structure, which
is a framework for organizing documentation into four distinct categories:

- **[Tutorials](https://diataxis.fr/tutorials/)**
    > A tutorial is an experience that takes place under the guidance of a
    > tutor. A tutorial is always learning-oriented.
- **[How-to Guides](https://diataxis.fr/how-to-guides/)**
    > How-to guides are directions that guide the reader through a problem or
    > towards a result. How-to guides are goal-oriented.
- **[Explanation](https://diataxis.fr/explanation/)**
    > Explanation is a discursive treatment of a subject, that permits
    > reflection. Explanation is understanding-oriented.
- **[Reference](https://diataxis.fr/reference/)**
    > Reference guides are technical descriptions of the machinery and how to
    > operate it. Reference material is information-oriented.

As such, docs exist in the following directories:

- `/docs/Tutorials`
- `/docs/How-To-Guides`
- `/docs/Explanation`
- `/docs/Reference`

:::warning NOTICE

It is strongly suggested to read the
[diataxis summary available here](https://diataxis.fr/start-here/).

:::

## Formatting

Use Markdown for all documentation files.

Recommended VSCode extensions for Markdown:

- linting: `DavidAnson.vscode-markdownlint`
- ToC and other helpers: `yzhang.markdown-all-in-one`
- Word wrap: `stkb.rewrap`

### Resources

Store all resources (e.g. images) in the `/docs/resources` directory.
When linking to resources, use relative paths to the `/docs/resources` directory.

### Diagrams

Prefer the use of mermaid diagrams embedded in markdown files. This allows for
easy editing and version control of diagrams.

```markdown
:::
flowchart TD
    A[Hard] -->|Text| B(Round)
    B --> C{Decision}
    C -->|One| D[Result 1]
    C -->|Two| E[Result 2]
:::
```

### Internal Links

When linking to other documents within the documentation, use relative paths.
For example, to link to a document in the same directory, use:

```markdown
[Link Text](./path/to/document.md)
```

## Document Structure

All documents should follow a consistent structure to ensure clarity and
readability. The format will vary a bit between the different types of
documents.

### Tutorial Structure

Recommended reading: [Diátaxis Tutorials](https://diataxis.fr/tutorials/)

[TEMPLATE](tutorial-template.md)

Tutorials as introductory guides for users to learn the basics of working with
Trident. As such, they should have very little jargon, prerequisites, or
expectations. Tutorials aim to get the users to learn by doing.

**⚠️ SAFETY:** Trident is a powerful tool and tutorials may be a user's first
experience with it. As such, it is important to **ensure that tutorials are safe
and do not lead to any unintended consequences!**

The following structure is recommended:

1. **Title**: A clear and descriptive title that reflects the content of the
    tutorial. For example, "Performing a deployment with Trident" or "Creating a
    Trident-ready image".
2. **Introduction**: A brief overview of the tutorial's purpose and what users
    will learn. This should inform a user if the tutorial is relevant to them
    and what they can expect to achieve by the end of it.
3. **Prerequisites**: A list of any prerequisites or requirements for the
    tutorial. This may include software versions, hardware requirements, or
    any other necessary tools or information.
4. **Instructions**: A clear and concise set of instructions
    for completing the tutorial. Use numbered lists or bullet points to
    break down complex tasks into manageable steps. Include code snippets,
    screenshots, or diagrams as needed to illustrate the process.
5. **Conclusion**: A summary of what the user has learned and any next steps
    they can take. This may include links to related tutorials, documentation,
    or resources for further learning.

### How-To Guides Structure

Recommended reading: [Diátaxis How-To Guides](https://diataxis.fr/how-to-guides/)

[TEMPLATE](how-to-guide-template.md)

How-to guides are more advanced than tutorials and are intended for users who
are already familiar with Trident and want to learn how to perform specific
tasks or solve specific problems. They should be more technical and may include
more jargon or advanced concepts. How-to guides should provide clear,
step-by-step instructions for completing a task, along with any necessary
background information or context. They should also include troubleshooting tips
and common pitfalls to avoid.

**⚠️ SAFETY:** How-to guides may be a user's first experience with a new
feature. As such, it is important to **ensure that how-to guides are safe, or
have clear warnings about possible risks or side effects otherwise!**

The following structure is recommended:

1. **Title**: A clear and descriptive title that reflects the content of the
    how-to guide. For example, "Configure Root Verity" or "Validate Host Configuration".
2. **Goals**: A brief an concise list of the specific goals that a user will
    achieve if they follow this guide.
3. **Prerequisites**: A list of any prerequisites or requirements for the
    how-to guide. This may include software versions, hardware requirements, or
    any other necessary tools or information.
4. **Instructions**: A clear and concise set of instructions for completing the
    how-to guide. Use numbered lists or bullet points to break down complex
    tasks into manageable steps. Include code snippets, screenshots, or diagrams
    as needed to illustrate the process.
5. **Troubleshooting**: A section that provides troubleshooting tips and common
    pitfalls to avoid. This may include links to related documentation or
    resources for further learning.

### Explanation Structure

Recommended reading: [Diátaxis Explanation](https://diataxis.fr/explanation/)

Explanation documents are intended to provide a deeper understanding of
Trident's features, concepts, and architecture. They should be more technical
and may include more jargon or advanced concepts. They should also include
diagrams (use mermaid!), code snippets, or other illustrations as needed to help
explain complex concepts whenever possible.

In Trident, the best use of this section is to cover these types of questions:

- "Why does Trident do X?"
- "How does Trident do X?"
- "How does Trident work?"
- "How does X work?"
- "How does X work in Trident?"
- "How is X feature implemented in Trident?"

In an ideal world, whenever we get any of these questions, we can simply reply
with a link to the explanation page on "X".

The following structure is recommended:

1. **Title**: A clear and descriptive title that reflects the content of the
    explanation. It should generally be the name of the feature or concept
    being explained. For example, "Root Verity" or "Trident Architecture".
2. **Overview**: A brief overview of the feature or concept being explained.
    This should include a high-level description of what it is and why it is
    important.
3. (Whatever is needed to explain the feature or concept)

### Reference Structure

Recommended reading: [Diátaxis Reference](https://diataxis.fr/reference/)

Reference docs are information heavy technical descriptions such as API
documentation. We rarely write this type of doc. We can evaluate new additions
on a case-by-case basis.
