use std::{cmp::max, collections::HashMap};

use anyhow::{bail, Context, Error};
use svg::{
    node::element::{Group, Path, Rectangle, Style, TSpan, Text},
    Document, Node,
};
use textwrap::Options;

use super::{
    nodes::{DiagramNode, Legend},
    Diagram,
};

const MARGIN: u32 = 10;
const AXIS_WIDTH: u32 = 30;
const DEFAULT_FILL: &str = "#008492";
const DEFAULT_STROKE: &str = "#003F46";
const DEFAULT_TEXT: &str = "#FFFFFF";

struct NodeGroup {
    group: Group,
    height: u32,
    width: u32,
}

type Legends<'a> = HashMap<&'a str, &'a Legend>;

pub(super) fn render(diagram: Diagram) -> Result<Document, Error> {
    let legends = generate_legends(&diagram);
    let axis_legend = time_axis().translated(legends.width as i32, 0);

    let annotations = Group::new()
        .add(axis_legend)
        .add(legends.group.translated(MARGIN as i32, MARGIN as i32));

    let legend_map = diagram
        .legends
        .iter()
        .map(|legend| (legend.id.as_str(), legend))
        .collect::<Legends>();

    let root_node = render_children(&legend_map, diagram.root.iter())?
        .translated((MARGIN + legends.width + AXIS_WIDTH) as i32, MARGIN as i32);

    let width = MARGIN + legends.width + AXIS_WIDTH + root_node.width + MARGIN;
    let height = root_node.height + 2 * MARGIN;

    Ok(Document::new()
        .set("width", width)
        .set("height", height)
        .set("font-family", "Aptos,Aptos_MSFontService,sans-serif")
        .set("viewBox", (0, 0, width, height))
        .add(Style::new(".caption { fill: white; }"))
        .add(
            Rectangle::new()
                .set("x", 0)
                .set("y", 0)
                .set("width", "100%")
                .set("height", "100%")
                .set("fill", "white")
                .set("stroke", "#000000")
                .set("stroke-width", 2),
        )
        .add(annotations)
        .add(root_node.group))
}

fn render_children<'a>(
    legends: &Legends,
    children: impl Iterator<Item = &'a DiagramNode>,
) -> Result<NodeGroup, Error> {
    let mut children_group = Group::new();
    let mut y_pos: u32 = 0;
    let mut child_width: u32 = 0;
    for child in children {
        if has_children(&children_group) {
            y_pos += MARGIN; // Add some space between nodes
        }

        let child_node = render_node(legends, child)?.translated(0, y_pos as i32);
        child_width = max(child_width, child_node.width);

        y_pos += child_node.height;
        children_group.append(child_node.group);
    }

    Ok(NodeGroup {
        group: children_group,
        height: y_pos,
        width: child_width,
    })
}

fn render_node(legends: &Legends, node: &DiagramNode) -> Result<NodeGroup, Error> {
    if node.comment.is_some() && !node.children.is_empty() {
        bail!(
            "Only leaf nodes can have comments, but {} has both",
            node.name
        );
    }

    let box_width = 100;

    let children =
        render_children(legends, node.children.iter())?.translated((box_width + MARGIN) as i32, 0);

    let wrapped_name = textwrap::wrap(&node.name, Options::new(20));
    let text_height = wrapped_name.len() as u32 * 10; // Approximate height of text

    let height = max(children.height, text_height + 10);

    let mut self_box = Rectangle::new()
        .set("x", 0)
        .set("y", 0)
        .set("width", box_width)
        .set("height", height);

    let mut self_text = Text::new("")
        .set("x", box_width / 2)
        .set("y", height / 2 - (wrapped_name.len() - 1) as u32 * 5)
        .set("text-anchor", "middle")
        .set("dominant-baseline", "middle")
        .set("class", "caption");

    apply_legend(legends, node, &mut self_box, &mut self_text)
        .with_context(|| format!("Failed to apply legend for node {}", node.name))?;

    for (i, line) in wrapped_name.iter().enumerate() {
        let mut span = TSpan::new(line.to_string())
            .set("x", box_width / 2)
            .set("text-anchor", "middle")
            .set("dominant-baseline", "middle")
            .set("font-size", "10px");
        if i > 0 {
            span.assign("dy", "12px");
        }
        self_text = self_text.add(span);
    }

    let mut width = box_width;

    let mut self_group = Group::new().add(self_box).add(self_text);

    // If we have children, add a margin and translate the group
    // to the right of the box with.
    if has_children(&children.group) {
        width += MARGIN + children.width;
        self_group.append(children.group);
    }

    if let Some(comment) = &node.comment {
        width += MARGIN;
        let comment_text = Text::new(comment.clone())
            .set("x", width)
            .set("y", height / 2)
            .set("text-anchor", "start")
            .set("dominant-baseline", "middle")
            .set("font-size", "7px");
        self_group = self_group.add(comment_text);
        width += comment.len() as u32 * 10 / 3; // Approximate width of comment
    }

    Ok(NodeGroup {
        group: self_group,
        height,
        width,
    })
}

/// Trait to add transformations to SVG elements.
trait Transform {
    fn translated(self, x: i32, y: i32) -> Self;
}

impl Transform for Group {
    fn translated(self, x: i32, y: i32) -> Self {
        self.set("transform", format!("translate({}, {})", x, y))
    }
}

impl Transform for NodeGroup {
    fn translated(self, x: i32, y: i32) -> Self {
        NodeGroup {
            group: self.group.translated(x, y),
            height: self.height,
            width: self.width,
        }
    }
}

fn generate_legends(diagram: &Diagram) -> NodeGroup {
    let mut legend_width = 0;
    let mut legend_y = 0;
    let mut legend_group = Group::new();
    let square_size = 10;
    let text_start = square_size + 2;
    for legend in &diagram.legends {
        let rect = Rectangle::new()
            .set("x", 0)
            .set("y", 0)
            .set("width", square_size)
            .set("height", square_size)
            .set("stroke-width", 2)
            .set("fill", legend.background.as_deref().unwrap_or(DEFAULT_FILL))
            .set("stroke", legend.border.as_deref().unwrap_or(DEFAULT_STROKE));

        let display_text = legend.friendly.as_ref().unwrap_or(&legend.id);

        let text = Text::new(display_text)
            .set("x", text_start)
            .set("y", square_size / 2)
            .set("text-anchor", "start")
            .set("dominant-baseline", "middle")
            .set("font-size", "10px");

        legend_group.append(
            Group::new()
                .add(rect)
                .add(text)
                .translated(0, legend_y as i32),
        );

        legend_y += square_size + MARGIN;
        legend_width = max(
            legend_width,
            text_start + display_text.len() as u32 * 12 / 3 + text_start + MARGIN,
        );
    }

    NodeGroup {
        group: legend_group,
        height: legend_y,
        width: legend_width,
    }
}

fn time_axis() -> Group {
    Group::new()
        .add(
            Path::new().set("d", "M1641.94 1436.5 1641.94 1838.22 1635.06 1838.22 1635.06 1436.5ZM1652.25 1833.63 1638.5 1861.13 1624.75 1833.63Z")
            .set("fill", "#7F7F7F")
            .set("transform", "translate(-145,-135) scale(0.1)"),
        )
        .add(
            Text::new("Time")
                .set("fill", "#7F7F7F")
                .set("font-size", "10px")
                .set("text-anchor", "start")
                .set("transform", "translate(15,30) rotate(-90)")
            )
}

fn apply_legend(
    legends: &Legends,
    node: &DiagramNode,
    rect: &mut Rectangle,
    text: &mut Text,
) -> Result<(), Error> {
    let legend = node
        .legend
        .as_ref()
        .map(|id| {
            legends.get(id.as_str()).with_context(|| {
                format!("Legend with ID '{}' not found for node {}", id, node.name)
            })
        })
        .transpose()?;

    if let Some(legend) = legend {
        rect.assign("fill", legend.background.as_deref().unwrap_or(DEFAULT_FILL));
        rect.assign("stroke", legend.border.as_deref().unwrap_or(DEFAULT_STROKE));
        text.assign("fill", legend.text.as_deref().unwrap_or(DEFAULT_TEXT));
    } else {
        rect.assign("fill", DEFAULT_FILL);
        rect.assign("stroke", DEFAULT_STROKE);
        text.assign("fill", DEFAULT_TEXT);
    }

    Ok(())
}

fn has_children(node: &impl Node) -> bool {
    match node.get_children() {
        Some(children) => !children.is_empty(),
        None => false,
    }
}
