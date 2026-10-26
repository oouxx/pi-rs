//! Agent 定义发现 —— 对齐 pi-subagents / pi 上游
//! `examples/extensions/subagent/agents.ts`。
//!
//! Agent 定义是带 YAML frontmatter 的 markdown 文件：
//!
//! ```markdown
//! ---
//! name: reviewer
//! description: Code review
//! tools: read, grep, find, bash
//! model: claude-sonnet-4-6
//! ---
//!
//! System prompt for the agent goes here.
//! ```
//!
//! 搜索位置（`AgentScope` 控制）：
//! - 用户级 `{agent_dir}/agents/*.md`（始终加载，除非 scope = project）
//! - 项目级 `<最近的祖先目录>/.pi-rs/agents/*.md`（scope = project/both）

use std::path::{Path, PathBuf};

use serde_yaml::Value as Yaml;

/// 项目级配置目录名。对齐 TS `CONFIG_DIR_NAME`（pi 为 `.pi`，pi-rs 为
/// `.pi-rs`，见 `pi_coding_agent::config::CONFIG_DIR_NAME`；此处独立成常量
/// 以免 pi-extensions 反向依赖 pi-coding-agent）。
pub const PROJECT_CONFIG_DIR: &str = ".pi-rs";

/// Agent 搜索范围（对齐 TS `AgentScope`）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AgentScope {
    /// 仅用户级（默认）。
    #[default]
    User,
    /// 仅项目级。
    Project,
    /// 两者都加载；同名项目 agent 覆盖用户 agent。
    Both,
}

impl AgentScope {
    /// 解析工具参数里的 `agentScope`；未知值返回 `None`（调用方按默认处理）。
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "user" => Some(Self::User),
            "project" => Some(Self::Project),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
            Self::Both => "both",
        }
    }
}

/// Agent 定义来源。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentSource {
    /// `{agent_dir}/agents`。
    User,
    /// `<cwd 祖先>/.pi-rs/agents`。
    Project,
}

impl AgentSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Project => "project",
        }
    }
}

/// 一个 agent 定义（对齐 TS `AgentConfig`）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentConfig {
    pub name: String,
    pub description: String,
    /// 工具白名单；`None` = 子 agent 用 pi 默认工具集。
    pub tools: Option<Vec<String>>,
    /// 模型覆盖；`None` = 继承派发会话的模型与 thinking level。
    pub model: Option<String>,
    /// frontmatter 之后的 markdown 正文，作为子 agent 的 system prompt。
    pub system_prompt: String,
    pub source: AgentSource,
    pub file_path: String,
}

/// 发现结果（对齐 TS `AgentDiscoveryResult`）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentDiscovery {
    pub agents: Vec<AgentConfig>,
    /// 项目级 agent 目录（用于诊断/确认弹窗）；未找到时为 `None`。
    pub project_agents_dir: Option<String>,
}

/// 发现 agent 定义。
///
/// `user_agents_dir` 由调用方给出（`{agent_dir}/agents`，agent_dir 来自
/// `runtime.get_agent_dir()`），这样测试和嵌入方可以指向任意目录。
#[must_use]
pub fn discover_agents(user_agents_dir: &Path, cwd: &str, scope: AgentScope) -> AgentDiscovery {
    let project_agents_dir = find_nearest_project_agents_dir(Path::new(cwd));

    let mut agents: Vec<AgentConfig> = Vec::new();
    if scope != AgentScope::Project {
        agents.extend(load_agents_from_dir(user_agents_dir, AgentSource::User));
    }
    if scope != AgentScope::User {
        if let Some(dir) = project_agents_dir.as_deref() {
            for agent in load_agents_from_dir(Path::new(dir), AgentSource::Project) {
                // TS 用 Map 按 name 去重（后者覆盖前者），保留首次插入位置。
                if let Some(existing) = agents.iter_mut().find(|a| a.name == agent.name) {
                    *existing = agent;
                } else {
                    agents.push(agent);
                }
            }
        }
    }

    AgentDiscovery {
        agents,
        project_agents_dir,
    }
}

/// 按名字查 agent 定义。
#[must_use]
pub fn find_agent<'a>(agents: &'a [AgentConfig], name: &str) -> Option<&'a AgentConfig> {
    agents.iter().find(|a| a.name == name)
}

/// 把 agent 列表压成一行摘要（对齐 TS `formatAgentList`）。
///
/// 返回 `(摘要文本, 未列出数量)`；空列表返回 `("none", 0)`。
#[must_use]
pub fn format_agent_list(agents: &[AgentConfig], max_items: usize) -> (String, usize) {
    if agents.is_empty() {
        return ("none".to_string(), 0);
    }
    let listed = agents.len().min(max_items);
    let text = agents
        .iter()
        .take(listed)
        .map(|a| format!("{} ({}): {}", a.name, a.source.as_str(), a.description))
        .collect::<Vec<_>>()
        .join("; ");
    (text, agents.len() - listed)
}

/// 加载目录下所有 `*.md` agent 定义（对齐 TS `loadAgentsFromDir`）。
///
/// 单个坏文件必须跳过而不是让整个目录失败。
fn load_agents_from_dir(dir: &Path, source: AgentSource) -> Vec<AgentConfig> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut agents = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        // TS: `if (!entry.isFile() && !entry.isSymbolicLink()) continue;`
        let is_file_or_link = entry
            .file_type()
            .map(|t| t.is_file() || t.is_symlink())
            .unwrap_or(false);
        if !is_file_or_link {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };

        let (frontmatter, body) = parse_frontmatter(&content);
        let (Some(name), Some(description)) = (
            yaml_str(&frontmatter, "name"),
            yaml_str(&frontmatter, "description"),
        ) else {
            continue;
        };

        agents.push(AgentConfig {
            name: name.to_string(),
            description: description.to_string(),
            tools: parse_tool_list(frontmatter.get("tools")),
            model: yaml_str(&frontmatter, "model").map(str::to_string),
            system_prompt: body,
            source,
            file_path: path.to_string_lossy().into_owned(),
        });
    }

    // TS 依赖 `readdirSync` 顺序（文件系统相关）；这里排序以获得确定性
    // 输出（只影响列表展示顺序，不影响查找）。
    agents.sort_by(|a, b| a.name.cmp(&b.name));
    agents
}

/// 从最近的祖先目录向上查找 `.pi-rs/agents`（对齐
/// TS `findNearestProjectAgentsDir`）。
fn find_nearest_project_agents_dir(cwd: &Path) -> Option<String> {
    let mut current: PathBuf = if cwd.as_os_str().is_empty() {
        std::env::current_dir().ok()?
    } else {
        cwd.to_path_buf()
    };
    loop {
        let candidate = current.join(PROJECT_CONFIG_DIR).join("agents");
        if candidate.is_dir() {
            return Some(candidate.to_string_lossy().into_owned());
        }
        if !current.pop() {
            return None;
        }
    }
}

/// 把 frontmatter 的 `tools` 归一为工具名列表。
///
/// YAML 两种写法都合法且都在用：`tools: read, bash` / `tools: [read, bash]`。
/// 其他类型（数字、映射、嵌套列表）返回 `None` 而不是报错。
fn parse_tool_list(value: Option<&Yaml>) -> Option<Vec<String>> {
    let raw: Vec<&str> = match value? {
        Yaml::Sequence(items) => items.iter().filter_map(Yaml::as_str).collect(),
        Yaml::String(s) => s.split(',').collect(),
        _ => return None,
    };
    let tools: Vec<String> = raw
        .into_iter()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .collect();
    (!tools.is_empty()).then_some(tools)
}

fn yaml_str<'a>(mapping: &'a serde_yaml::Mapping, key: &str) -> Option<&'a str> {
    mapping.get(key).and_then(Yaml::as_str)
}

/// 解析 markdown frontmatter（对齐 TS `utils/frontmatter.ts`）：
/// 去 BOM、CRLF→LF，要求以 `---` 开头，`\n---` 结束；正文 trim。
///
/// 无 frontmatter / 未闭合 / YAML 解析失败时返回空 mapping + 全文正文。
fn parse_frontmatter(content: &str) -> (serde_yaml::Mapping, String) {
    let normalized = content
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n")
        .replace('\r', "\n");

    if !normalized.starts_with("---") {
        return (serde_yaml::Mapping::new(), normalized);
    }
    let Some(end_index) = normalized[3..].find("\n---").map(|i| i + 3) else {
        return (serde_yaml::Mapping::new(), normalized);
    };

    // TS: yamlString = normalized.slice(4, endIndex)
    let yaml_string = normalized.get(4..end_index).unwrap_or_default();
    // TS: body = normalized.slice(endIndex + 4).trim()
    let body = normalized
        .get(end_index + 4..)
        .unwrap_or_default()
        .trim()
        .to_string();

    let mapping = serde_yaml::from_str::<Yaml>(yaml_string)
        .ok()
        .and_then(|v| match v {
            Yaml::Mapping(m) => Some(m),
            _ => None,
        })
        .unwrap_or_default();

    (mapping, body)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn write_agent(dir: &Path, file: &str, content: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(file), content).unwrap();
    }

    #[test]
    fn frontmatter_parsed_like_ts() {
        let (fm, body) = parse_frontmatter(
            "---\nname: reviewer\ndescription: Code review\ntools: read, bash\n---\n\nYou review.\n",
        );
        assert_eq!(yaml_str(&fm, "name"), Some("reviewer"));
        assert_eq!(yaml_str(&fm, "description"), Some("Code review"));
        assert_eq!(body, "You review.");
    }

    #[test]
    fn frontmatter_handles_bom_and_crlf() {
        let (fm, body) = parse_frontmatter("\u{feff}---\r\nname: a\r\n---\r\nB\r\n");
        assert_eq!(yaml_str(&fm, "name"), Some("a"));
        assert_eq!(body, "B");
    }

    #[test]
    fn frontmatter_absent_falls_back_to_body() {
        let (fm, body) = parse_frontmatter("just text");
        assert!(fm.is_empty());
        assert_eq!(body, "just text");
    }

    #[test]
    fn frontmatter_unclosed_falls_back_to_body() {
        let (fm, body) = parse_frontmatter("---\nname: a\n");
        assert!(fm.is_empty());
        assert_eq!(body, "---\nname: a\n");
    }

    #[test]
    fn tool_list_accepts_string_and_array() {
        let (fm, _) = parse_frontmatter("---\ntools: read, bash\n---\n");
        assert_eq!(
            parse_tool_list(fm.get("tools")),
            Some(vec!["read".to_string(), "bash".to_string()])
        );
        let (fm, _) = parse_frontmatter("---\ntools: [read, bash]\n---\n");
        assert_eq!(
            parse_tool_list(fm.get("tools")),
            Some(vec!["read".to_string(), "bash".to_string()])
        );
        let (fm, _) = parse_frontmatter("---\ntools: 3\n---\n");
        assert_eq!(parse_tool_list(fm.get("tools")), None);
    }

    #[test]
    fn loads_agents_and_skips_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        write_agent(
            &agents_dir,
            "reviewer.md",
            "---\nname: reviewer\ndescription: Review\nmodel: m1\n---\nbody",
        );
        // 缺 description → 跳过
        write_agent(&agents_dir, "broken.md", "---\nname: broken\n---\nbody");
        // 非 md → 跳过
        write_agent(
            &agents_dir,
            "notes.txt",
            "---\nname: x\ndescription: y\n---\n",
        );

        let found = load_agents_from_dir(&agents_dir, AgentSource::User);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "reviewer");
        assert_eq!(found[0].model.as_deref(), Some("m1"));
        assert_eq!(found[0].system_prompt, "body");
        assert_eq!(found[0].source, AgentSource::User);
    }

    #[test]
    fn project_agents_override_user_agents_in_both_scope() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let user_dir = home.path().join("agents");
        write_agent(
            &user_dir,
            "worker.md",
            "---\nname: worker\ndescription: user worker\n---\nuser",
        );
        write_agent(
            &user_dir,
            "scout.md",
            "---\nname: scout\ndescription: user scout\n---\nuser",
        );
        write_agent(
            &project.path().join(PROJECT_CONFIG_DIR).join("agents"),
            "worker.md",
            "---\nname: worker\ndescription: project worker\n---\nproject",
        );

        let cwd = project.path().to_string_lossy().to_string();
        let both = discover_agents(&user_dir, &cwd, AgentScope::Both);
        assert_eq!(both.agents.len(), 2);
        let worker = find_agent(&both.agents, "worker").unwrap();
        assert_eq!(worker.source, AgentSource::Project);
        assert_eq!(worker.system_prompt, "project");

        // 默认 scope = user：项目 agent 不加载
        let user_only = discover_agents(&user_dir, &cwd, AgentScope::User);
        assert_eq!(user_only.agents.len(), 2);
        assert!(user_only
            .agents
            .iter()
            .all(|a| a.source == AgentSource::User));
        assert!(user_only.project_agents_dir.is_some(), "still reports dir");

        // scope = project：只剩项目 agent
        let project_only = discover_agents(&user_dir, &cwd, AgentScope::Project);
        assert_eq!(project_only.agents.len(), 1);
        assert_eq!(project_only.agents[0].source, AgentSource::Project);
    }

    #[test]
    fn project_agents_dir_found_from_nested_cwd() {
        let root = tempfile::tempdir().unwrap();
        let nested = root.path().join("a").join("b");
        std::fs::create_dir_all(nested.join(PROJECT_CONFIG_DIR).join("agents")).unwrap();
        let found = find_nearest_project_agents_dir(&nested);
        assert_eq!(
            found,
            Some(
                nested
                    .join(PROJECT_CONFIG_DIR)
                    .join("agents")
                    .to_string_lossy()
                    .into_owned()
            )
        );
    }

    #[test]
    fn format_agent_list_reports_remaining() {
        let agents = vec![
            AgentConfig {
                name: "a".into(),
                description: "A".into(),
                tools: None,
                model: None,
                system_prompt: String::new(),
                source: AgentSource::User,
                file_path: String::new(),
            },
            AgentConfig {
                name: "b".into(),
                description: "B".into(),
                tools: None,
                model: None,
                system_prompt: String::new(),
                source: AgentSource::Project,
                file_path: String::new(),
            },
        ];
        assert_eq!(format_agent_list(&[], 3), ("none".to_string(), 0));
        assert_eq!(
            format_agent_list(&agents, 1),
            ("a (user): A".to_string(), 1)
        );
    }
}
