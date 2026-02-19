mod common;

use mcp_git::server::{
    CommitParams, DiffParams, LogParams, McpGitServer, RepoEntry, RepoParam, SearchParams,
};

fn make_server(repo: &common::TestRepo) -> McpGitServer {
    let entry = RepoEntry {
        name: repo.name(),
        path: repo.path(),
    };
    McpGitServer::new(vec![entry], 500, 50)
}

fn make_multi_server(repos: &[&common::TestRepo]) -> McpGitServer {
    let entries: Vec<RepoEntry> = repos
        .iter()
        .map(|r| RepoEntry {
            name: r.name(),
            path: r.path(),
        })
        .collect();
    McpGitServer::new(entries, 500, 50)
}

fn extract_text(result: rmcp::model::CallToolResult) -> serde_json::Value {
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    serde_json::from_str(&text).unwrap_or(serde_json::Value::Null)
}

#[test]
fn test_list_repos() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let result = server.do_list_repos().expect("list_repos failed");
    let json = extract_text(result);
    let arr = json.as_array().expect("should be array");

    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["name"], repo.name());
    assert_eq!(arr[0]["branch"], "main");
}

#[test]
fn test_log_basic() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let params = LogParams {
        repo: None,
        max_count: None,
        branch: None,
        author: None,
    };
    let result = server.do_log(params).expect("log failed");
    let json = extract_text(result);

    assert_eq!(json["count"], 3);
    let commits = json["commits"].as_array().unwrap();
    // Most recent first
    assert!(commits[0]["message"].as_str().unwrap().contains("lib.rs"));
    assert!(commits[2]["message"].as_str().unwrap().contains("README"));
}

#[test]
fn test_log_with_max_count() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let params = LogParams {
        repo: None,
        max_count: Some(2),
        branch: None,
        author: None,
    };
    let result = server.do_log(params).expect("log failed");
    let json = extract_text(result);

    assert_eq!(json["count"], 2);
}

#[test]
fn test_log_with_author_filter() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    // On main branch, all commits are by Alice — filter for Bob should return 0
    let params = LogParams {
        repo: None,
        max_count: Some(50),
        branch: None,
        author: Some("Bob".to_string()),
    };
    let result = server.do_log(params).expect("log failed");
    let json = extract_text(result);
    assert_eq!(json["count"], 0);

    // On feature branch, there's one commit by Bob
    let params = LogParams {
        repo: None,
        max_count: Some(50),
        branch: Some("feature".to_string()),
        author: Some("Bob".to_string()),
    };
    let result = server.do_log(params).expect("log failed");
    let json = extract_text(result);
    assert_eq!(json["count"], 1);
    assert!(json["commits"][0]["author"]
        .as_str()
        .unwrap()
        .contains("Bob"));
}

#[test]
fn test_log_author_filter_does_not_reduce_max() {
    // Regression test: author filter should not count filtered-out commits toward max
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    // feature branch has 4 commits: 3 by Alice + 1 by Bob
    // With max=50 and author=Alice, we should get all 3 Alice commits
    let params = LogParams {
        repo: None,
        max_count: Some(50),
        branch: Some("feature".to_string()),
        author: Some("Alice".to_string()),
    };
    let result = server.do_log(params).expect("log failed");
    let json = extract_text(result);
    assert_eq!(
        json["count"], 3,
        "Should find all 3 Alice commits on feature branch"
    );
}

#[test]
fn test_show_commit() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    // First get a commit SHA from the log
    let log_params = LogParams {
        repo: None,
        max_count: Some(1),
        branch: None,
        author: None,
    };
    let log_result = server.do_log(log_params).expect("log failed");
    let log_json = extract_text(log_result);
    let sha = log_json["commits"][0]["sha"].as_str().unwrap().to_string();

    let params = CommitParams {
        repo: None,
        commit: sha.clone(),
    };
    let result = server.do_show_commit(params).expect("show_commit failed");
    let json = extract_text(result);

    assert_eq!(json["sha"], sha);
    assert!(json["author"].as_str().unwrap().contains("Alice"));
    assert!(json["message"].as_str().is_some());
    assert!(json["parents"].as_array().is_some());
}

#[test]
fn test_list_branches() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let params = RepoParam { repo: None };
    let result = server
        .do_list_branches(params)
        .expect("list_branches failed");
    let json = extract_text(result);

    let local = json["local"].as_array().unwrap();
    let names: Vec<&str> = local.iter().map(|b| b["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"main"), "should contain main");
    assert!(names.contains(&"feature"), "should contain feature");

    // main should be marked as current
    let main_branch = local.iter().find(|b| b["name"] == "main").unwrap();
    assert_eq!(main_branch["current"], true);
}

#[test]
fn test_search_commits() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let params = SearchParams {
        repo: None,
        query: "README".to_string(),
        max_count: None,
    };
    let result = server
        .do_search_commits(params)
        .expect("search_commits failed");
    let json = extract_text(result);

    assert_eq!(json["count"], 1);
    assert!(json["matches"][0]["message"]
        .as_str()
        .unwrap()
        .contains("README"));
}

#[test]
fn test_diff() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    // Get first and last commit SHAs on main
    let log_params = LogParams {
        repo: None,
        max_count: Some(10),
        branch: None,
        author: None,
    };
    let log_result = server.do_log(log_params).expect("log failed");
    let log_json = extract_text(log_result);
    let commits = log_json["commits"].as_array().unwrap();

    let newest = commits[0]["sha"].as_str().unwrap().to_string();
    let oldest = commits.last().unwrap()["sha"].as_str().unwrap().to_string();

    let params = DiffParams {
        repo: None,
        from_ref: oldest,
        to_ref: Some(newest),
        path: None,
    };
    let result = server.do_diff(params).expect("diff failed");
    let json = extract_text(result);

    assert!(
        json["file_count"].as_u64().unwrap() > 0,
        "should have changed files"
    );
    let files = json["files"].as_array().unwrap();
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert!(paths.contains(&"src/main.rs"), "should include src/main.rs");
    assert!(paths.contains(&"src/lib.rs"), "should include src/lib.rs");
}

#[test]
fn test_diff_with_path_filter() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let log_params = LogParams {
        repo: None,
        max_count: Some(10),
        branch: None,
        author: None,
    };
    let log_result = server.do_log(log_params).expect("log failed");
    let log_json = extract_text(log_result);
    let commits = log_json["commits"].as_array().unwrap();
    let newest = commits[0]["sha"].as_str().unwrap().to_string();
    let oldest = commits.last().unwrap()["sha"].as_str().unwrap().to_string();

    let params = DiffParams {
        repo: None,
        from_ref: oldest,
        to_ref: Some(newest),
        path: Some("src/main".to_string()),
    };
    let result = server.do_diff(params).expect("diff failed");
    let json = extract_text(result);

    let files = json["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "src/main.rs");
}

#[test]
fn test_resolve_single_repo() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    // With single repo, None should work
    let params = RepoParam { repo: None };
    let result = server.do_list_branches(params);
    assert!(
        result.is_ok(),
        "resolve(None) should succeed with single repo"
    );
}

#[test]
fn test_resolve_ambiguous() {
    let repo1 = common::TestRepo::new();
    let repo2 = common::TestRepo::new();
    let server = make_multi_server(&[&repo1, &repo2]);

    // With multiple repos, None should fail
    let params = RepoParam { repo: None };
    let result = server.do_list_branches(params);
    assert!(
        result.is_err(),
        "resolve(None) should fail with multiple repos"
    );
}

#[test]
fn test_resolve_not_found() {
    let repo = common::TestRepo::new();
    let server = make_server(&repo);

    let params = RepoParam {
        repo: Some("nonexistent".to_string()),
    };
    let result = server.do_list_branches(params);
    assert!(result.is_err(), "resolve with wrong name should fail");
}
