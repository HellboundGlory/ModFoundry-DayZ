//! Reads the style names and fonts out of a `.styles` file.

use std::collections::BTreeSet;

/// The `Name` attribute of every `<Style ...>` tag in a `.styles` file.
pub fn style_names(text: &str) -> BTreeSet<String> {
    let text = strip_xml_comments(text);
    let mut names = BTreeSet::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("<Style").map(|at| at + "<Style".len()) {
        let after = &rest[at..];
        if !after.starts_with(char::is_whitespace) {
            rest = after;
            continue;
        }
        let Some(tag) = tag_attributes(after) else {
            rest = after;
            continue;
        };
        if let Some(name) = attribute(tag, "Name").filter(|name| !name.is_empty()) {
            names.insert(name.to_string());
        }
        rest = &after[tag.len()..];
    }
    names
}

/// The `(widget, style, font)` of every `<Style>` with a non-empty `Font`, under the `<Widget Name>` it sits in.
pub fn style_fonts(text: &str) -> Vec<(String, String, String)> {
    style_attribute(text, "Font")
}

/// The `(widget, style, color)` of every `<Style>` with a numeric `Color`, under the `<Widget Name>` it sits in.
pub fn style_colors(text: &str) -> Vec<(String, String, u32)> {
    style_attribute(text, "Color")
        .into_iter()
        .filter_map(|(widget, style, color)| Some((widget, style, color.parse().ok()?)))
        .collect()
}

/// The `ImageSet` of a style and the non-empty images of its `Normal` state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StyleImages {
    pub imageset: String,
    /// `(slot, image)` in file order: the `Name` and `Image` of each `<Item>` whose `Image` is not empty.
    pub slots: Vec<(String, String)>,
}

/// The `(widget, style, images)` of every named `<Style>` under a `<Widget Name>`.
pub fn style_images(text: &str) -> Vec<(String, String, StyleImages)> {
    let text = strip_xml_comments(text);
    let mut rows: Vec<(String, String, StyleImages)> = Vec::new();
    let mut widget = String::new();
    let mut open_style = None;
    let mut in_normal = false;
    for (name, tag) in tags(&text) {
        if let Some(closing) = tag.strip_prefix('/') {
            match closing.trim() {
                "Style" => open_style = None,
                "State" => in_normal = false,
                _ => {}
            }
            continue;
        }
        let self_closing = tag.trim_end().ends_with('/');
        match name {
            "Widget" => widget = attribute(tag, "Name").unwrap_or_default().to_string(),
            "Style" => {
                in_normal = false;
                open_style = None;
                let Some(style) = attribute(tag, "Name").filter(|style| !style.is_empty()) else {
                    continue;
                };
                let imageset = attribute(tag, "ImageSet").unwrap_or_default().to_string();
                rows.push((
                    widget.clone(),
                    style.to_string(),
                    StyleImages {
                        imageset,
                        slots: Vec::new(),
                    },
                ));
                if !self_closing {
                    open_style = Some(rows.len() - 1);
                }
            }
            "State" => in_normal = !self_closing && attribute(tag, "Name") == Some("Normal"),
            "Item" if in_normal => {
                let (Some(index), Some(image)) = (
                    open_style,
                    attribute(tag, "Image").filter(|image| !image.is_empty()),
                ) else {
                    continue;
                };
                let slot = attribute(tag, "Name").unwrap_or_default().to_string();
                rows[index].2.slots.push((slot, image.to_string()));
            }
            _ => {}
        }
    }
    rows
}

fn style_attribute(text: &str, key: &str) -> Vec<(String, String, String)> {
    let text = strip_xml_comments(text);
    let mut found = Vec::new();
    let mut widget = String::new();
    for (name, tag) in tags(&text) {
        match name {
            "Widget" => widget = attribute(tag, "Name").unwrap_or_default().to_string(),
            "Style" => {
                let style = attribute(tag, "Name").unwrap_or_default();
                let value = attribute(tag, key).unwrap_or_default();
                if !style.is_empty() && !value.is_empty() {
                    found.push((widget.clone(), style.to_string(), value.to_string()));
                }
            }
            _ => {}
        }
    }
    found
}

/// The name and attribute text of each tag, stopping at the first tag that never closes.
fn tags(text: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = text;
    std::iter::from_fn(move || {
        let at = rest.find('<')?;
        let after = &rest[at + 1..];
        let name_len = after
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(after.len());
        let (name, body) = after.split_at(name_len);
        let tag = tag_attributes(body)?;
        rest = &body[tag.len()..];
        Some((name, tag))
    })
}

/// Removes `<!-- ... -->` spans, leaving anything outside them.
fn strip_xml_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("<!--") {
        out.push_str(&rest[..at]);
        match rest[at + 4..].find("-->") {
            Some(end) => rest = &rest[at + 4 + end + 3..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The text of one tag up to its closing `>`, quotes respected; `None` when the tag never closes.
fn tag_attributes(text: &str) -> Option<&str> {
    let mut quote: Option<char> = None;
    for (at, c) in text.char_indices() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '>' => return Some(&text[..at]),
            None => {}
        }
    }
    None
}

/// The value of the attribute named exactly `name` in a tag's text.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(at) = rest.find(name) {
        let after = &rest[at + name.len()..];
        let starts_word = rest[..at]
            .chars()
            .next_back()
            .is_none_or(char::is_whitespace);
        let value = after.trim_start().strip_prefix('=').map(str::trim_start);
        if let Some(value) = value.filter(|_| starts_word) {
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'');
            if let Some(quote) = quote {
                return value[1..].split(quote).next();
            }
        }
        rest = after;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{style_colors, style_fonts, style_images, StyleImages};

    #[test]
    fn style_images_reads_the_normal_state_of_each_style() {
        let text = r#"<Widgets>
  <!-- <Widget Name="Old"><Style Name="Fake" ImageSet="old"><State Name="Normal"><Item Name="Center" Image="Old"/></State></Style></Widget> -->
  <Widget Name="PanelWidget">
    <Style Name="Default" ImageSet="dayz_gui" Color="4294967295">
      <State Name="Disabled">
        <Item Name="Center" Image="DisabledCenter" />
      </State>
      <State Name="Normal">
        <Item Name="Bottom" Image="DayZDefaultPanelBottom" />
        <Item Name="Center" Image="" />
        <Item Name="Left" Image="DayZDefaultPanelLeft" />
      </State>
    </Style>
    <Style Name="Empty" ImageSet="dayz_gui">
      <State Name="Disabled">
        <Item Name="Center" Image="DisabledOnly" />
      </State>
    </Style>
    <Style Name="Blank" ImageSet="dayz_gui"/>
  </Widget>
  <Widget Name="WindowWidget">
    <Style Name="Default" ImageSet="dayz_gui">
      <State Name="Normal">
        <Item Name="Top" Image="WindowTop" />
      </State>
    </Style>
  </Widget>
</Widgets>"#;

        let images = |imageset: &str, slots: &[(&str, &str)]| StyleImages {
            imageset: imageset.to_string(),
            slots: slots
                .iter()
                .map(|(slot, image)| (slot.to_string(), image.to_string()))
                .collect(),
        };
        assert_eq!(
            style_images(text),
            vec![
                (
                    "PanelWidget".to_string(),
                    "Default".to_string(),
                    images(
                        "dayz_gui",
                        &[
                            ("Bottom", "DayZDefaultPanelBottom"),
                            ("Left", "DayZDefaultPanelLeft"),
                        ]
                    ),
                ),
                (
                    "PanelWidget".to_string(),
                    "Empty".to_string(),
                    images("dayz_gui", &[]),
                ),
                (
                    "PanelWidget".to_string(),
                    "Blank".to_string(),
                    images("dayz_gui", &[]),
                ),
                (
                    "WindowWidget".to_string(),
                    "Default".to_string(),
                    images("dayz_gui", &[("Top", "WindowTop")]),
                ),
            ]
        );
    }

    #[test]
    fn style_colors_reads_the_colour_of_each_styled_widget() {
        let text = r#"<Widgets>
  <!-- <Widget Name="Old"><Style Name="Colorable" Color="4294967295"/></Widget> -->
  <Widget Name="WrapSpacerWidget">
    <Style Name="Colorable" Font="gui/fonts/Old" Color="0"/>
    <Style Name="Plain" Font="gui/fonts/Old"/>
    <Style Name="Text" TextColor="7"/>
  </Widget>
  <Widget Name="WindowWidget">
    <Style Name="Colorable" Color="4294967295"/>
  </Widget>
</Widgets>"#;

        assert_eq!(
            style_colors(text),
            vec![
                ("WrapSpacerWidget".to_string(), "Colorable".to_string(), 0),
                (
                    "WindowWidget".to_string(),
                    "Colorable".to_string(),
                    4_294_967_295
                ),
            ]
        );
    }

    #[test]
    fn style_fonts_names_the_font_of_each_styled_widget() {
        let text = r#"<Widgets>
  <!-- <Widget Name="Old"><Style Name="Normal" Font="gui/fonts/Old"/></Widget> -->
  <Widget Name="TextWidget">
    <Style Name="Normal" Font="gui/fonts/MetronBook"/>
    <Style Name="Plain"/>
  </Widget>
</Widgets>"#;

        assert_eq!(
            style_fonts(text),
            vec![(
                "TextWidget".to_string(),
                "Normal".to_string(),
                "gui/fonts/MetronBook".to_string(),
            )]
        );
    }
}
