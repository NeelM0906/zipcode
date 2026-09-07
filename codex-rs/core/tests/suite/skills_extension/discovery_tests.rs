use super::*;
use pretty_assertions::assert_eq;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_code_mode_bounds_host_list_pages() -> Result<()> {
    assert_production_host_pages("list").await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn production_code_mode_bounds_host_read_pages() -> Result<()> {
    assert_production_host_pages("read").await
}

async fn assert_production_host_pages(operation: &str) -> Result<()> {
    let server = responses::start_mock_server().await;
    let codex_home = Arc::new(TempDir::new()?);
    let names = (0..24)
        .map(|i| format!("bounded-host-{i:02}"))
        .collect::<Vec<_>>();
    let description = "d".repeat(1_000);
    write_host_skills(
        codex_home.path(),
        &names
            .iter()
            .map(|name| (name.as_str(), description.as_str()))
            .collect::<Vec<_>>(),
    )?;
    let skill_path =
        dunce::canonicalize(codex_home.path().join("skills/bounded-host-00/SKILL.md"))?;
    let header = std::fs::read_to_string(&skill_path)?;
    let body = "é🦀\"\\\n";
    std::fs::write(&skill_path, format!("{header}{}", body.repeat(3_000)))?;
    let resource = skill_path.to_string_lossy().into_owned();
    let code = format!(
        r#"
const operation = {operation};
const resource = {resource};
const expected = {header} + {body}.repeat(3000);
let cursor = null, pages = 0, maxBytes = 0, contents = "", names = [], warnings = [];
for (let i = 0; i < 100; i++) {{
  const page = operation === "list"
    ? await tools.skills__list({{authority: {{kind: "host"}}, query: "bounded-host", cursor}})
    : await tools.skills__read({{package: resource, cursor}});
  // Count UTF-8 bytes, not JS UTF-16 code units, including JSON escaping.
  const bytes = encodeURIComponent(JSON.stringify(page)).replace(/%[0-9A-F]{{2}}/g, "_").length;
  maxBytes = Math.max(maxBytes, bytes);
  pages++;
  if (operation === "list") {{
    names.push(...page.skills.map(skill => skill.name));
    warnings.push(...page.warnings);
  }} else {{ contents += page.contents; }}
  cursor = page.next_cursor;
  if (cursor === null) break;
}}
text({{maxBytes, pages, complete: cursor === null, names, warnings, contentsMatch: operation === "list" || contents === expected}});
"#,
        operation = json!(operation),
        resource = json!(resource),
        header = json!(header),
        body = json!(body)
    );
    let response = responses::mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-pages"),
                responses::ev_custom_tool_call("host-pages", "exec", &code),
                ev_completed("resp-pages"),
            ]),
            sse(vec![
                ev_response_created("resp-done"),
                ev_completed("resp-done"),
            ]),
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
        .with_code_mode_host_program(codex_utils_cargo_bin::cargo_bin("codex-code-mode-host")?)
        .with_extensions(Arc::new(extensions.build()))
        .with_model_info_override("gpt-5.5", |model_info| {
            model_info.tool_mode = Some(codex_protocol::openai_models::ToolMode::CodeMode);
            model_info.truncation_policy = TruncationPolicyConfig::bytes(/*limit*/ 512 * 1024);
        })
        .with_config(|config| {
            configure_catalog_test(config);
            config.skill_max_context_tokens = std::num::NonZeroUsize::new(1);
            config
                .features
                .enable(Feature::CodeMode)
                .expect("Code Mode should be configurable");
        });
    let test = builder.build_with_auto_env(&server).await?;
    test.submit_turn("Use Code Mode to page through the admitted host skills.")
        .await?;
    let requests = response.requests();
    assert_eq!(requests.len(), 2);
    let output = requests[1].custom_tool_call_output("host-pages");
    let report = output["output"]
        .as_array()
        .and_then(|items| items.last())
        .and_then(|item| item["text"].as_str())
        .ok_or_else(|| {
            anyhow::anyhow!("Code Mode should return its actual host page measurements: {output}")
        })?;
    let mut report: Value = serde_json::from_str(report)?;
    let max_bytes = report["maxBytes"]
        .as_u64()
        .expect("measured response byte count");
    assert!(
        max_bytes <= 8_000,
        "host {operation} response exceeded 8,000 bytes: {max_bytes}"
    );
    assert!(
        report["pages"].as_u64().is_some_and(|pages| pages > 1),
        "large host {operation} should paginate"
    );
    report.as_object_mut().unwrap().remove("maxBytes");
    report.as_object_mut().unwrap().remove("pages");
    assert_eq!(
        report,
        json!({"complete": true, "names": if operation == "list" {names} else {Vec::new()}, "warnings": [], "contentsMatch": true})
    );
    Ok(())
}

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
