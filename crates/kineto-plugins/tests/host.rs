//! The host against the demo plugin binary: answers, checked files, a
//! cleared environment, and every way a plugin can misbehave.

use std::{path::PathBuf, time::Duration};

use kineto_plugins::{Answer, FailureClass, HostError, Input, Limits, PluginEntry, run};
use serde_json::json;

fn entry(mode: Option<&str>) -> PluginEntry {
    PluginEntry {
        id: "demo".to_owned(),
        executable: PathBuf::from(env!("CARGO_BIN_EXE_kineto-demo-plugin")),
        args: mode.map(str::to_owned).into_iter().collect(),
        secrets: vec!["DEMO_KEY".to_owned()],
        enabled: true,
    }
}

fn secret(name: &str) -> Option<String> {
    (name == "DEMO_KEY").then(|| "s3cret".to_owned())
}

fn invoke(mode: Option<&str>, limits: Limits) -> Result<kineto_plugins::Done, HostError> {
    run(
        &entry(mode),
        &secret,
        "invoke",
        json!({"capability": "image.generate", "idempotency_key": "image.generate:a:sha256:0",
               "request": {"prompt": "A filmmaker.", "aspect_ratio": "16:9"}}),
        &[Input {
            name: "identity-0.jpg".to_owned(),
            bytes: vec![0xff, 0xd8, 0xff],
        }],
        limits,
    )
}

#[test]
fn a_plugin_describes_itself_and_makes_an_image_it_declares() {
    let described = run(
        &entry(None),
        &secret,
        "describe",
        json!({}),
        &[],
        Limits::QUICK,
    )
    .unwrap();
    let Answer::Ok { body } = described.answer else {
        panic!("{:?}", described.answer)
    };
    assert_eq!(
        body["capabilities"]["image.generate"]["max_reference_images"],
        2
    );

    let done = invoke(None, Limits::INVOKE).unwrap();
    assert!(matches!(done.answer, Answer::Completed { .. }));
    let [image] = done.files.as_slice() else {
        panic!("{:?}", done.files)
    };
    assert_eq!(image.declared.path, "outputs/image.jpg");
    assert!(image.bytes.starts_with(&[0xff, 0xd8, 0xff]));
    // The same request makes the same image.
    assert_eq!(
        invoke(None, Limits::INVOKE).unwrap().files[0].bytes,
        image.bytes
    );
}

#[test]
fn a_plugin_sees_only_its_environment_its_secret_and_its_scratch_directory() {
    let done = invoke(Some("--env"), Limits::INVOKE).unwrap();
    let Answer::Completed { metadata, .. } = done.answer else {
        panic!()
    };
    let names: Vec<&str> = metadata["env"]
        .as_array()
        .unwrap()
        .iter()
        .map(|name| name.as_str().unwrap())
        .collect();
    assert_eq!(names, ["DEMO_KEY", "HOME", "KINETO_PLUGIN_DIR", "PATH"]);
    let cwd = metadata["cwd"].as_str().unwrap();
    assert!(cwd.contains("kineto-plugin-"), "{cwd}");
    assert!(
        !std::path::Path::new(cwd).exists(),
        "the scratch directory is gone after the call"
    );
    assert_eq!(metadata["inputs"][0]["path"], "inputs/identity-0.jpg");
    assert_eq!(metadata["inputs"][0]["size"], 3);
}

#[test]
fn a_plugin_that_misbehaves_fails_the_call() {
    let short = Limits {
        deadline: Duration::from_millis(400),
        ..Limits::QUICK
    };
    assert!(matches!(
        invoke(Some("--sleep"), short),
        Err(HostError::Timeout)
    ));
    assert!(matches!(
        invoke(Some("--crash"), Limits::QUICK),
        Err(HostError::Exited(Some(3)))
    ));
    assert!(matches!(
        invoke(Some("--garbage"), Limits::QUICK),
        Err(HostError::Unreadable(_))
    ));
    let wrong = invoke(Some("--wrong-hash"), Limits::QUICK).unwrap_err();
    assert!(
        wrong.to_string().contains("not the file declared"),
        "{wrong}"
    );
    let tiny = Limits {
        max_answer: 8,
        ..Limits::QUICK
    };
    assert!(
        invoke(None, tiny)
            .unwrap_err()
            .to_string()
            .contains("more than 8 bytes")
    );

    // Files each within the size limit are still refused, before any is
    // read, when there are too many or they are too large together.
    let many = invoke(Some("--many"), Limits::QUICK).unwrap();
    assert_eq!(many.files.len(), 3);
    let size = many.files[0].declared.size;
    let few = Limits {
        max_outputs: 2,
        ..Limits::QUICK
    };
    let error = invoke(Some("--many"), few).unwrap_err();
    assert!(error.to_string().contains("names 3 files"), "{error}");
    let small = Limits {
        max_file: size,
        max_total: 3 * size - 1,
        ..Limits::QUICK
    };
    let error = invoke(Some("--many"), small).unwrap_err();
    assert!(error.to_string().contains("together"), "{error}");

    // A plugin that never ran never saw the request; the rest may have.
    let missing = PluginEntry {
        executable: PathBuf::from("/nonexistent/kineto-plugin"),
        ..entry(None)
    };
    let error = run(&missing, &secret, "describe", json!({}), &[], Limits::QUICK).unwrap_err();
    assert!(error.unsent(), "{error}");
    assert!(!HostError::Timeout.unsent());

    // Failures are answers: only one the provider certainly did not take
    // says so.
    let Answer::Failed {
        class, accepted, ..
    } = invoke(Some("--refuse"), Limits::QUICK).unwrap().answer
    else {
        panic!()
    };
    assert_eq!((class, accepted), (FailureClass::Terminal, Some(false)));
    let Answer::Failed { accepted, .. } = invoke(Some("--fail"), Limits::QUICK).unwrap().answer
    else {
        panic!()
    };
    assert_eq!(accepted, None);
}
