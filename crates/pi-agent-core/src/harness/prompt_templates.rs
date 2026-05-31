use crate::harness::types::PromptTemplate;

pub fn format_prompt_template_invocation(template: &PromptTemplate, args: &[String]) -> String {
    substitute_args(&template.content, args)
}

pub fn substitute_args(content: &str, args: &[String]) -> String {
    let mut result = content.to_string();

    let re_num = regex::Regex::new(r"\$(\d+)").unwrap();
    let args_owned: Vec<String> = args.to_vec();
    result = re_num
        .replace_all(&result, |caps: &regex::Captures| {
            let num: usize = caps[1].parse().unwrap_or(0);
            args_owned
                .get(num.saturating_sub(1))
                .cloned()
                .unwrap_or_default()
        })
        .to_string();

    let re_range = regex::Regex::new(r"\$\{@:(\d+)(?::(\d+))?\}").unwrap();
    let args_owned2: Vec<String> = args.to_vec();
    result = re_range
        .replace_all(&result, |caps: &regex::Captures| {
            let start: usize = caps[1].parse::<usize>().unwrap_or(1).saturating_sub(1);
            let end: Option<usize> = caps.get(2).and_then(|m| m.as_str().parse::<usize>().ok());
            match end {
                Some(e) => args_owned2
                    .get(start..e.min(args_owned2.len()))
                    .map(|slice: &[String]| slice.join(" "))
                    .unwrap_or_default(),
                None => args_owned2
                    .get(start..)
                    .map(|slice: &[String]| slice.join(" "))
                    .unwrap_or_default(),
            }
        })
        .to_string();

    let all_args = args.join(" ");
    result = result.replace("$ARGUMENTS", &all_args);
    result = result.replace("$@", &all_args);

    result
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::types::PromptTemplate;

    #[test]
    fn test_substitute_args_positional() {
        let content = "Hello $1, welcome $2";
        let args = vec!["world".to_string(), "home".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello world, welcome home");
    }

    #[test]
    fn test_substitute_args_missing_positional() {
        let content = "Hello $1, $2";
        let args = vec!["world".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello world, ");
    }

    #[test]
    fn test_substitute_args_arguments_variable() {
        let content = "Review $1 with $ARGUMENTS";
        let args = vec!["a.ts".to_string(), "care".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Review a.ts with a.ts care");
    }

    #[test]
    fn test_substitute_args_at_variable() {
        let content = "All: $@";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "All: a b c");
    }

    #[test]
    fn test_substitute_args_range_from() {
        let content = "${@:2}";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "b c");
    }

    #[test]
    fn test_substitute_args_range_with_end() {
        let content = "${@:1:2}";
        let args = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "a b");
    }

    #[test]
    fn test_substitute_args_range_out_of_bounds() {
        let content = "${@:5}";
        let args = vec!["a".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "");
    }

    #[test]
    fn test_substitute_args_combined() {
        let content = "$1 ${@:2} $ARGUMENTS";
        let args = vec!["hello".to_string(), "world".to_string(), "test".to_string()];
        let result = substitute_args(content, &args);
        assert_eq!(result, "hello world test hello world test");
    }

    #[test]
    fn test_substitute_args_no_placeholders() {
        let content = "No placeholders here";
        let args: Vec<String> = vec![];
        let result = substitute_args(content, &args);
        assert_eq!(result, "No placeholders here");
    }

    #[test]
    fn test_substitute_args_empty_args() {
        let content = "Hello $1";
        let args: Vec<String> = vec![];
        let result = substitute_args(content, &args);
        assert_eq!(result, "Hello ");
    }

    #[test]
    fn test_format_prompt_template_invocation() {
        let template = PromptTemplate {
            name: "review".to_string(),
            description: "Review code".to_string(),
            content: "Review $1 with $ARGUMENTS".to_string(),
        };
        let args = vec!["a.ts".to_string(), "care".to_string()];
        let result = format_prompt_template_invocation(&template, &args);
        assert_eq!(result, "Review a.ts with a.ts care");
    }

    #[test]
    fn test_parse_command_args_simple() {
        let result = parse_command_args("hello world");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_quoted() {
        let result = parse_command_args("hello \"world test\"");
        assert_eq!(result, vec!["hello", "world test"]);
    }

    #[test]
    fn test_parse_command_args_single_quoted() {
        let result = parse_command_args("hello 'world test'");
        assert_eq!(result, vec!["hello", "world test"]);
    }

    #[test]
    fn test_parse_command_args_empty() {
        let result = parse_command_args("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_command_args_extra_spaces() {
        let result = parse_command_args("  hello   world  ");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_tabs() {
        let result = parse_command_args("hello\tworld");
        assert_eq!(result, vec!["hello", "world"]);
    }

    #[test]
    fn test_parse_command_args_mixed_quotes() {
        let result = parse_command_args("hello \"world 'test'\"");
        assert_eq!(result, vec!["hello", "world 'test'"]);
    }

    #[test]
    fn test_parse_command_args_unclosed_quote() {
        let result = parse_command_args("hello \"world");
        assert_eq!(result, vec!["hello", "world"]);
    }
}