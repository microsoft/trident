use serde::{de::Visitor, Deserialize, Deserializer};

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(super) struct Diagram {
    #[serde(default, deserialize_with = "deserialize_legends")]
    pub legends: Vec<Legend>,

    pub root: Vec<DiagramNode>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(super) struct Legend {
    #[serde(skip)]
    pub id: String,

    #[serde(default)]
    pub friendly: Option<String>,

    #[serde(default)]
    pub background: Option<String>,

    #[serde(default)]
    pub border: Option<String>,

    #[serde(default)]
    pub text: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub(super) struct DiagramNode {
    pub name: String,

    #[serde(default)]
    pub children: Vec<DiagramNode>,

    #[serde(default)]
    pub comment: Option<String>,

    #[serde(default)]
    pub legend: Option<String>,
}

fn deserialize_legends<'de, D>(deserializer: D) -> Result<Vec<Legend>, D::Error>
where
    D: Deserializer<'de>,
{
    struct LegendMapVisitor;
    impl<'de> Visitor<'de> for LegendMapVisitor {
        type Value = Vec<Legend>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map of legends")
        }

        fn visit_map<V>(self, mut map: V) -> Result<Self::Value, V::Error>
        where
            V: serde::de::MapAccess<'de>,
        {
            let mut legends = Vec::new();
            while let Some((key, value)) = map.next_entry::<String, Legend>()? {
                let mut legend = value;
                legend.id = key;
                legends.push(legend);
            }
            Ok(legends)
        }
    }

    deserializer
        .deserialize_map(LegendMapVisitor)
        .map_err(serde::de::Error::custom)
}
