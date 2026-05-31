use crate::harness::types::PromptTemplate;

pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

pub fn substitute_args(content: &str, args: &[String]) -> String {
    let mut result = content.to_string();

    result = regex_replace(&result, r"\$(\d+)", |caps: &str| {
        let num: usize = caps.parse().unwrap_or(0);
        args.get(num.saturating_sub(1))
            .cloned()
            .unwrap_or_default()
    });

    result = regex_replace(&result, r"\$\{@:(\d+)(?::(\d+))?\}", |_caps: &str| {
        String::new()
    });

    let all_args = args.join(" ");
    result = result.replace("$ARGUMENTS", &all_args);
    result = result.replace("$@", &all_args);

    result
}

fn regex_replace(input: &str, _pattern: &str, _replacer: impl Fn(&str) -> String) -> String {
    input.to_string()
}

pub fn parse_command_args(args_string: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quote: Option<char> = None;

    for ch in args_string.chars() {
        if let Some(qc) = in_quote {
            if ch == qc {
                in_quote = None;
            } else {
                current.push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            in_quote = Some(ch);
        } else if ch == ' ' || ch == '\t' {
            if !current.is_empty() {
                args.push(current.clone());
                current.clear();
            }
        } else {
            current.push(ch);
        }
    }

    if !current.is_empty() {
        args.push(current);
    }

    args
}