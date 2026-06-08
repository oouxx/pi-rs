use crate::harness::types::Skill;

pub fn format_skills_for_system_prompt(skills: &[Skill]) -> String {
    let visible_skills: Vec<_> = skills
        .iter()
        .filter(|s| !s.disable_model_invocation)
        .collect();

    if visible_skills.is_empty() {
        return String::new();
    }

    let mut lines = vec![
        "The following skills provide specialized instructions for specific tasks.".to_string(),
        "Read the full skill file when the task matches its description.".to_string(),
        "When a skill file references a relative path resolve it against the skill directory (parent of SKILL.md / dirname of the path) and use that absolute path in tool commands.".to_string(),
        String::new(),
        "<available_skills>".to_string(),
    ];

    for skill in visible_skills {
        lines.push("  <skill>".to_string());
        lines.push(format!("    <name>{}</name>", escape_xml(&skill.name)));
        lines.push(format!(
            "    <description>{}</description>",
            escape_xml(&skill.description)
        ));
        lines.push(format!(
            "    <location>{}</location>",
            escape_xml(&skill.file_path)
        ));
        lines.push("  </skill>".to_string());
    }

    lines.push("</available_skills>".to_string());
    lines.join("\n")
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_skills_visible_and_hidden() {
        let visible_skill = Skill {
            name: "visible".to_string(),
            description: "Use <this> & that".to_string(),
            content: "visible content".to_string(),
            file_path: "/skills/visible/SKILL.md".to_string(),
            disable_model_invocation: false,
        };
        let second_skill = Skill {
            name: "second".to_string(),
            description: "Second skill".to_string(),
            content: "second content".to_string(),
            file_path: "/skills/second/SKILL.md".to_string(),
            disable_model_invocation: false,
        };
        let disabled_skill = Skill {
            name: "hidden".to_string(),
            description: "Hidden".to_string(),
            content: "hidden content".to_string(),
            file_path: "/skills/hidden/SKILL.md".to_string(),
            disable_model_invocation: true,
        };

        let result =
            format_skills_for_system_prompt(&[visible_skill.clone(), disabled_skill, second_skill]);

        assert!(result.contains("<available_skills>"));
        assert!(result.contains("<name>visible</name>"));
        assert!(result.contains("<description>Use &lt;this&gt; &amp; that</description>"));
        assert!(result.contains("<location>/skills/visible/SKILL.md</location>"));
        assert!(result.contains("<name>second</name>"));
        assert!(result.contains("<description>Second skill</description>"));
        assert!(result.contains("<location>/skills/second/SKILL.md</location>"));
        assert!(!result.contains("<name>hidden</name>"));
        assert!(result.contains("</available_skills>"));
    }

    #[test]
    fn test_format_skills_empty_when_all_disabled() {
        let disabled_skill = Skill {
            name: "hidden".to_string(),
            description: "Hidden".to_string(),
            content: "hidden content".to_string(),
            file_path: "/skills/hidden/SKILL.md".to_string(),
            disable_model_invocation: true,
        };
        let result = format_skills_for_system_prompt(&[disabled_skill]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_skills_empty_input() {
        let result = format_skills_for_system_prompt(&[]);
        assert!(result.is_empty());
    }

    #[test]
    fn test_format_skills_xml_escaping() {
        let skill = Skill {
            name: "a&b".to_string(),
            description: "Quote \"double\" and 'single'".to_string(),
            content: "content".to_string(),
            file_path: "/skills/<bad>&\"quote\"/SKILL.md".to_string(),
            disable_model_invocation: false,
        };
        let result = format_skills_for_system_prompt(&[skill]);
        assert!(result.contains("<name>a&amp;b</name>"));
        assert!(result.contains(
            "<description>Quote &quot;double&quot; and &apos;single&apos;</description>"
        ));
        assert!(result
            .contains("<location>/skills/&lt;bad&gt;&amp;&quot;quote&quot;/SKILL.md</location>"));
    }
}
