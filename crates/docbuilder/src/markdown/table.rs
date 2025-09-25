use itertools::Itertools;

pub struct MdTable {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl MdTable {
    pub fn new(headers: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: Vec::new(),
        }
    }

    pub fn with_rows(
        mut self,
        new_rows: impl IntoIterator<Item = impl IntoIterator<Item = impl Into<String>>>,
    ) -> Self {
        new_rows.into_iter().for_each(|row| self.add_row(row));
        self
    }

    pub fn add_row(&mut self, row: impl IntoIterator<Item = impl Into<String>>) {
        let row: Vec<String> = row.into_iter().map(Into::into).collect();
        if row.len() != self.headers.len() {
            panic!("Row length does not match header length");
        }
        self.rows.push(row);
    }

    pub fn render(&self) -> String {
        let mut table = String::new();
        // let mut widths = self.headers.iter().zip()

        // Calculate column widths
        let widths = self
            .headers
            .iter()
            .enumerate()
            .map(|(i, header)| {
                std::cmp::max(
                    header.len(),
                    self.rows
                        .iter()
                        .map(|row| row[i].len())
                        .max()
                        .unwrap_or_default(),
                )
            })
            .collect::<Vec<usize>>();

        // Render headers
        table.push_str("| ");
        table.push_str(
            &self
                .headers
                .iter()
                .zip(widths.iter())
                .map(|(header, width)| format!("{header:<width$}"))
                .join(" | "),
        );
        table.push_str(" |\n");

        // Render separator
        table.push_str("| ");
        table.push_str(&widths.iter().map(|width| "-".repeat(*width)).join(" | "));
        table.push_str(" |\n");

        // Render rows
        for row in self.rows.iter() {
            table.push_str("| ");
            table.push_str(
                &row.iter()
                    .zip(widths.iter())
                    .map(|(header, width)| format!("{header:<width$}"))
                    .join(" | "),
            );
            table.push_str(" |\n");
        }

        table
    }
}
