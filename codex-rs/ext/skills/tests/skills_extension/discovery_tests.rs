use super::*;
use codex_extension_api::ToolExecutor;
use codex_skills::SkillPolicy;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;

async fn invoke(
    tool: &Arc<dyn for<'call> ToolExecutor<ToolCall<'call>>>,
    arguments: Value,
) -> Result<Value, FunctionCallError> {
    let payload = ToolPayload::Function {
        arguments: arguments.to_string(),
    };
    let output = tool
        .handle(ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "discovery".to_string(),
            tool_name: tool.tool_name(),
            model: "gpt-test".to_string(),
            codex_turn_metadata: None,
            truncation_policy: TruncationPolicy::Bytes(10_000),
            source: ToolCallSource::Direct,
            conversation_history: ConversationHistory::default(),
            turn_item_emitter: Arc::new(NoopTurnItemEmitter),
            environments: Vec::new(),
            payload: payload.clone(),
        })
        .await?;
    output
        .post_tool_use_response("discovery", &payload)
        .ok_or_else(|| {
            FunctionCallError::Fatal("skill tool should expose structured output".to_string())
        })
}

#[tokio::test]
async fn host_discovery_respects_host_instruction_opt_out() -> TestResult {
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(
        &mut builder,
        SkillProviders::new().with_host_provider(Arc::new(HostSkillProvider::new())),
        skills_extension_config,
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let step_store = ExtensionData::new("turn-1");
    step_store.insert(HostSkillsSnapshot::new(Arc::new(
        SkillLoadOutcome::default(),
    )));
    let config = TestConfig {
        include_instructions: false,
        ..default_config()
    };
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;
    let tools =
        registry.tool_contributors()[0].tools_for_step(&session_store, &thread_store, &step_store);
    assert!(tools.is_empty());
    Ok(())
}

#[tokio::test]
async fn host_discovery_queries_full_snapshot_and_reads_only_selected_main_resource() -> TestResult
{
    let temp = tempfile::tempdir()?;
    let mut outcome = SkillLoadOutcome::default();
    for index in 0..24 {
        let directory = temp.path().join(format!("skill-{index:02}"));
        std::fs::create_dir(&directory)?;
        let path = directory.join("SKILL.md");
        std::fs::write(&path, format!("Host instructions {index}"))?;
        outcome.skills.push(SkillMetadata {
            name: format!("skill-{index:02}"),
            description: "Searchable astronomy instructions".to_string(),
            short_description: None,
            interface: None,
            dependencies: None,
            policy: None,
            path_to_skills_md: AbsolutePathBuf::try_from(path)?,
            scope: SkillScope::User,
            plugin_id: None,
            remote_plugin_id: None,
        });
    }
    outcome
        .disabled_paths
        .insert(outcome.skills[22].path_to_skills_md.clone());
    outcome.skills[23].policy = Some(SkillPolicy {
        allow_implicit_invocation: Some(false),
        ..Default::default()
    });
    let target = outcome.skills[21]
        .path_to_skills_md
        .to_string_lossy()
        .into_owned();
    let other = outcome.skills[22]
        .path_to_skills_md
        .to_string_lossy()
        .into_owned();
    let snapshot = HostSkillsSnapshot::new(Arc::new(outcome));
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(
        &mut builder,
        SkillProviders::new().with_host_provider(Arc::new(HostSkillProvider::new())),
        |config: &TestConfig| SkillsExtensionConfig {
            max_context_tokens: std::num::NonZeroUsize::new(100),
            ..skills_extension_config(config)
        },
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let step_store = ExtensionData::new("turn-1");
    let config = default_config();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;
    let contributor = &registry.tool_contributors()[0];
    assert!(
        contributor
            .tools_for_step(&session_store, &thread_store, &step_store)
            .is_empty()
    );
    step_store.insert(snapshot);
    let fragments = registry.turn_input_contributors()[0]
        .contribute(
            TurnInputContext {
                turn_id: "turn-1".to_string(),
                user_input: Vec::new(),
                environments: Vec::new(),
            },
            /*extension_metrics*/ None,
            &session_store,
            &thread_store,
            &step_store,
        )
        .await;
    let inline = fragments
        .iter()
        .map(|fragment| fragment.render())
        .collect::<String>();
    assert!(inline.contains("## Skills"));
    assert!(!inline.contains("skill-21"));
    let tools = contributor.tools_for_step(&session_store, &thread_store, &step_store);
    assert_eq!(tools.len(), 2);
    let list = tools
        .iter()
        .find(|tool| tool.tool_name().name == "list")
        .unwrap();
    let read = tools
        .iter()
        .find(|tool| tool.tool_name().name == "read")
        .unwrap();
    let found = invoke(
        list,
        json!({"authority": {"kind": "host"}, "query": "SKILL-21"}),
    )
    .await?;
    assert_eq!(
        found,
        json!({
            "skills": [{"authority": {"kind": "host"}, "package": target,
                "name": "skill-21", "description": "Searchable astronomy instructions",
                "main_resource": target}],
            "warnings": [], "next_cursor": null
        })
    );
    let first = invoke(
        list,
        json!({"authority": {"kind": "host"}, "query": "astronomy"}),
    )
    .await?;
    assert_eq!(first["skills"].as_array().map(Vec::len), Some(20));
    let cursor = first["next_cursor"].clone();
    let second = invoke(
        list,
        json!({"authority": {"kind": "host"}, "query": "astronomy", "cursor": cursor}),
    )
    .await?;
    assert_eq!(
        second["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|skill| skill["name"].clone())
            .collect::<Vec<_>>(),
        vec![json!("skill-20"), json!("skill-21")]
    );
    assert_eq!(second["next_cursor"], Value::Null);
    assert_eq!(
        invoke(
            list,
            json!({"authority": {"kind": "host"}, "query": "Searchable", "cursor": cursor})
        )
        .await
        .err(),
        Some(FunctionCallError::RespondToModel(
            "skills.list cursor is stale; restart from the first page".to_string()
        ))
    );
    for query in ["".to_string(), "x".repeat(257), "a\nb".to_string()] {
        assert!(
            invoke(list, json!({"authority": {"kind": "host"}, "query": query}))
                .await
                .is_err()
        );
    }
    assert_eq!(
        invoke(read, json!({"package": target})).await?,
        json!({
            "resource": target, "contents": "Host instructions 21", "next_cursor": null
        })
    );
    assert_eq!(
        invoke(read, json!({"package": target, "resource": target})).await?,
        json!({
            "resource": target, "contents": "Host instructions 21", "next_cursor": null
        })
    );
    for resource in [
        other.clone(),
        temp.path()
            .join("unloaded.md")
            .to_string_lossy()
            .into_owned(),
    ] {
        assert!(
            invoke(read, json!({"package": target, "resource": resource}))
                .await
                .is_err()
        );
    }
    assert!(invoke(read, json!({"package": other})).await.is_err());
    assert_eq!(
        invoke(list, json!({"authority": {"kind": "executor"}})).await?,
        json!({
            "skills": [], "warnings": [], "next_cursor": null
        })
    );
    Ok(())
}

#[tokio::test]
async fn host_read_resolves_normalized_windows_and_logical_display_aliases() -> TestResult {
    let cases = [
        (
            r"C:\skills\windows\SKILL.md",
            "C:/skills/windows/SKILL.md",
            "C:/skills",
            "r0/windows/SKILL.md",
        ),
        (
            "/canonical/logical/SKILL.md",
            "/logical/skills/logical/SKILL.md",
            "/logical/skills",
            "r1/logical/SKILL.md",
        ),
    ];
    let requests = Arc::new(Mutex::new(Vec::new()));
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(
        &mut builder,
        SkillProviders::new().with_host_provider(Arc::new(StaticSkillProvider {
            catalog: SkillCatalog {
                entries: cases
                    .iter()
                    .map(|(package, display, root, _)| {
                        test_entry(SkillSourceKind::Host, "host", package, package)
                            .with_display_path(*display)
                            .with_alias_root(*root)
                    })
                    .collect(),
                warnings: Vec::new(),
            },
            read_requests: Arc::clone(&requests),
            list_calls: None,
            fail_first_list: false,
        })),
        skills_extension_config,
    );
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let step_store = ExtensionData::new("turn-1");
    step_store.insert(HostSkillsSnapshot::new(Arc::new(
        SkillLoadOutcome::default(),
    )));
    let config = default_config();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            mcp_resource_client: None,
            extension_metrics: None,
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;
    let tools =
        registry.tool_contributors()[0].tools_for_step(&session_store, &thread_store, &step_store);
    let read = tools
        .iter()
        .find(|tool| tool.tool_name().name == "read")
        .unwrap();
    for (package, _, _, alias) in cases {
        assert_eq!(
            invoke(read, json!({"package": alias})).await?,
            json!({
                "resource": package, "contents": "# Lint Fix\n\nRun the formatter.", "next_cursor": null
            })
        );
    }
    assert_eq!(
        read_request_keys(&requests),
        cases
            .iter()
            .map(|(package, _, _, _)| (
                SkillAuthority::new(SkillSourceKind::Host, "host"),
                SkillPackageId(package.to_string()),
                SkillResourceId::new(*package),
            ))
            .collect::<Vec<_>>()
    );
    Ok(())
}
