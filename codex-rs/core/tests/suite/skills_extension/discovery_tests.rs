use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_step_tools_discover_and_read_admitted_host_skill() -> Result<()> {
    let server = responses::start_mock_server().await;
    let codex_home = Arc::new(TempDir::new()?);
    write_host_skills(
        codex_home.path(),
        &[("runtime-host", "Runtime discovery fixture.")],
    )?;
    let skill_path = dunce::canonicalize(codex_home.path().join("skills/runtime-host/SKILL.md"))?;
    let resource = skill_path.to_string_lossy().into_owned();
    let contents = std::fs::read_to_string(&skill_path)?;
    let response = responses::mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                responses::ev_function_call_with_namespace(
                    "discover-host",
                    "skills",
                    "list",
                    &json!({"authority": {"kind": "host"}, "query": "runtime-host"}).to_string(),
                ),
                ev_completed("resp-1"),
            ]),
            sse(vec![
                ev_response_created("resp-2"),
                responses::ev_function_call_with_namespace(
                    "read-host",
                    "skills",
                    "read",
                    &json!({"package": resource}).to_string(),
                ),
                ev_completed("resp-2"),
            ]),
            sse(vec![ev_response_created("resp-3"), ev_completed("resp-3")]),
        ],
    )
    .await;
    let mut extensions = ExtensionRegistryBuilder::new();
    install(&mut extensions, |config: &Config| SkillsExtensionConfig {
        include_instructions: config.include_skill_instructions,
        max_context_tokens: config.skill_max_context_tokens,
        bundled_skills_enabled: false,
        orchestrator_skills_enabled: false,
        shadow_selection_enabled: false,
    });
    let mut builder = test_codex()
        .with_home(codex_home)
        .with_extensions(Arc::new(extensions.build()))
        .with_config(|config| {
            configure_catalog_test(config);
            config.skill_max_context_tokens = std::num::NonZeroUsize::new(1);
        });
    let test = builder.build_with_auto_env(&server).await?;
    test.submit_turn("Discover the runtime skill and read its instructions.")
        .await?;

    let requests = response.requests();
    assert_eq!(requests.len(), 3);
    assert!(
        requests[0]
            .message_input_texts("developer")
            .iter()
            .all(|text| !text.contains("- runtime-host:"))
    );
    let listed = requests[1]
        .function_call_output_text("discover-host")
        .expect("production router should expose skills.list for the admitted host snapshot");
    assert_eq!(
        serde_json::from_str::<Value>(&listed)?,
        json!({
            "skills": [{"authority": {"kind": "host"}, "package": resource,
                "name": "runtime-host", "description": "Runtime discovery fixture.",
                "main_resource": resource}],
            "warnings": [], "next_cursor": null
        })
    );
    let read = requests[2].function_call_output_text("read-host").expect(
        "the next production step should retain the admitted host snapshot for skills.read",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&read)?,
        json!({
            "resource": resource, "contents": contents, "next_cursor": null
        })
    );
    Ok(())
}
