//! MCP prompts: the workflow templates an agent can ask the host
//! for. A prompt is text, not product logic — the registered command does the
//! work — and it is gated by that command's verdict, so a prompt this session
//! could not act on is not offered.

use serde_json::{json, Value};

/// One prompt template and the registered command it drives.
pub struct Prompt {
    pub name: &'static str,
    pub description: &'static str,
    pub command: &'static str,
    pub arguments: &'static [Argument],
    /// The user message, with `{argument}` placeholders.
    template: &'static str,
}

/// One `arguments` entry of a prompt: all of this server's prompt arguments are
/// required, because each is a placeholder the message cannot be missing.
pub struct Argument {
    pub name: &'static str,
    pub description: &'static str,
}

pub const PROMPTS: &[Prompt] = &[
    Prompt {
        name: "diagnose_last_run",
        description: "Explain the errors a DayZ client/server run left in its logs.",
        command: "diag.errors",
        arguments: &[Argument {
            name: "dir",
            description: "profiles or log dir of the run to read",
        }],
        template: "Read the DayZ run capture under `{dir}` with the `diag.errors` tool. \
                   For each file-mapped error, say what it means in Enforce Script terms \
                   and the smallest source change that would fix it.",
    },
    Prompt {
        name: "lint_module",
        description: "Lint an Enforce Script module and explain the findings.",
        command: "script.diagnostics",
        arguments: &[Argument {
            name: "path",
            description: ".c file or module directory to lint",
        }],
        template: "Lint the Enforce Script sources at `{path}` with the \
                   `script.diagnostics` tool, then explain each MF-* finding and the \
                   change that clears it.",
    },
    Prompt {
        name: "scaffold_mod",
        description: "Create a new DayZ script mod and validate what was written.",
        command: "project.scaffold",
        arguments: &[
            Argument {
                name: "name",
                description: "mod name (also the class prefix)",
            },
            Argument {
                name: "dir",
                description: "directory to create the mod in",
            },
        ],
        template: "Create a DayZ script mod named `{name}` in `{dir}` with the \
                   `project.scaffold` tool, then run `config.validate` over the result \
                   and report the files it wrote.",
    },
];

impl Prompt {
    /// The `prompts/get` result for one call: the required arguments must be
    /// present as strings, and every placeholder is filled from them.
    pub fn render(&self, arguments: &Value) -> Result<Value, String> {
        let mut values = Vec::with_capacity(self.arguments.len());
        for argument in self.arguments {
            let value = arguments
                .get(argument.name)
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    format!("prompt {} needs argument `{}`", self.name, argument.name)
                })?;
            values.push((argument.name, value));
        }
        Ok(json!({
            "description": self.description,
            "messages": [ {
                "role": "user",
                "content": { "type": "text", "text": self.fill(&values) },
            } ],
        }))
    }

    /// One pass over the template, so a value is never scanned for placeholders.
    fn fill(&self, values: &[(&str, &str)]) -> String {
        let mut text = String::with_capacity(self.template.len());
        let mut rest = self.template;
        while let Some(open) = rest.find('{') {
            text.push_str(&rest[..open]);
            let after = &rest[open + 1..];
            let filled = after.find('}').and_then(|close| {
                let value = values.iter().find(|(name, _)| *name == &after[..close])?.1;
                Some((value, close))
            });
            match filled {
                Some((value, close)) => {
                    text.push_str(value);
                    rest = &after[close + 1..];
                }
                None => {
                    text.push('{');
                    rest = after;
                }
            }
        }
        text.push_str(rest);
        text
    }

    /// The `prompts/list` entry.
    pub fn listing(&self) -> Value {
        let arguments: Vec<Value> = self
            .arguments
            .iter()
            .map(|a| {
                json!({
                    "name": a.name,
                    "description": a.description,
                    "required": true,
                })
            })
            .collect();
        json!({
            "name": self.name,
            "description": self.description,
            "arguments": arguments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_prompt_renders_every_placeholder_it_declares() {
        for prompt in PROMPTS {
            // A typo'd placeholder (`{mod_dir}` against a declared `dir`) would
            // ship literal braces in the prompt text; only rendering every
            // template with every declared argument catches that.
            let arguments: serde_json::Map<String, Value> = prompt
                .arguments
                .iter()
                .map(|a| (a.name.to_owned(), json!(format!("<{}>", a.name))))
                .collect();
            let rendered = prompt
                .render(&Value::Object(arguments))
                .unwrap_or_else(|e| panic!("{} did not render: {e}", prompt.name));
            let text = rendered["messages"][0]["content"]["text"].as_str().unwrap();
            assert!(
                !text.contains('{') && !text.contains('}'),
                "{} leaves a placeholder behind: {text}",
                prompt.name
            );
            for argument in prompt.arguments {
                assert!(
                    text.contains(&format!("<{}>", argument.name)),
                    "{} does not use its `{}` argument: {text}",
                    prompt.name,
                    argument.name
                );
            }
        }
    }

    #[test]
    fn a_missing_argument_is_reported_by_name() {
        let prompt = PROMPTS
            .iter()
            .find(|p| p.name == "scaffold_mod")
            .expect("scaffold_mod");
        let err = prompt.render(&json!({ "name": "MyMod" })).unwrap_err();
        assert!(err.contains("dir"), "{err}");
    }

    #[test]
    fn an_argument_value_is_not_rendered_again() {
        let prompt = PROMPTS
            .iter()
            .find(|p| p.name == "scaffold_mod")
            .expect("scaffold_mod");
        let rendered = prompt
            .render(&json!({ "name": "{dir}", "dir": "X" }))
            .unwrap();
        let text = rendered["messages"][0]["content"]["text"].as_str().unwrap();
        assert!(text.contains("named `{dir}` in `X`"), "{text}");
    }

    #[test]
    fn a_listing_declares_every_argument_as_required() {
        for prompt in PROMPTS {
            let listing = prompt.listing();
            assert_eq!(listing["name"], prompt.name);
            assert_eq!(
                listing["arguments"].as_array().unwrap().len(),
                prompt.arguments.len()
            );
            for argument in listing["arguments"].as_array().unwrap() {
                assert_eq!(argument["required"], true);
            }
        }
    }
}
