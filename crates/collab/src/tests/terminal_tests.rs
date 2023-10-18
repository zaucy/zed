use std::{ops::Deref, sync::Arc, time::Duration};

use call::ActiveCall;

use gpui::{executor::Deterministic, test::EmptyView, TestAppContext};
use serde_json::json;

use crate::tests::TestServer;

#[gpui::test(iterations = 10)]
async fn test_terminal_sharing(
    deterministic: Arc<Deterministic>,
    cx_a: &mut TestAppContext,
    cx_b: &mut TestAppContext,
) {
    let mut server = TestServer::start(&deterministic).await;
    let client_a = server.create_client(cx_a, "user_a").await;
    let client_b = server.create_client(cx_b, "user_b").await;
    cx_a.update(terminal::init);
    cx_b.update(terminal::init);
    server
        .create_room(&mut [(&client_a, cx_a), (&client_b, cx_b)])
        .await;
    let active_call_a = cx_a.read(ActiveCall::global);

    let window_a = cx_a.add_window(|_| EmptyView);
    client_a.fs().insert_tree("/root", json!({})).await;
    let (project_a, _) = client_a.build_local_project("/root", cx_a).await;

    let project_id = active_call_a
        .update(cx_a, |call, cx| call.share_project(project_a.clone(), cx))
        .await
        .unwrap();
    let window_b = cx_b.add_window(|_| EmptyView);
    let project_b = client_b.build_remote_project(project_id, cx_b).await;

    // Build a terminal in A
    let terminal_a = project_a
        .update(cx_a, |project, cx| {
            project.create_terminal(None, window_a.deref().clone(), cx)
        })
        .unwrap();

    // Share the terminal in A
    let terminal_a_remote_id = terminal_a
        .update(cx_a, |terminal, cx| terminal.share(cx))
        .await
        .unwrap();

    // Open the terminal in B
    let terminal_b = project_b
        .update(cx_b, |project, cx| {
            project.create_remote_terminal(terminal_a_remote_id, window_b.deref().clone(), cx)
        })
        .unwrap();

    // Type a character in A and wait for the terminal to produce a new frame
    const A_TYPING: &str = "--ABC--";
    terminal_a
        .update(cx_a, |terminal, _| {
            terminal.input(A_TYPING.to_string());
            terminal.wait_for_text(A_TYPING, Duration::from_secs(5))
        })
        .await
        .expect("Terminal a should have echoed content by now");

    // Run until parked so B can see the frame
    deterministic.run_until_parked();

    // Assert that A and B's terminal have the same content
    assert_eq!(
        terminal_a.read_with(cx_a, |terminal, _| terminal.last_content.cells.clone()),
        terminal_b.read_with(cx_b, |terminal, _| terminal.last_content.cells.clone()),
    );

    //Type a character in B wait for the terminal to produce a new frame
    const B_TYPING: &str = "++XYZ++";
    terminal_b
        .update(cx_b, |terminal, _| {
            terminal.input(B_TYPING.to_string());
            terminal.wait_for_text(B_TYPING, Duration::from_secs(5))
        })
        .await
        .expect("Terminal b should have echoed content by now");

    // Since B has seen the frame with this text, we know we don't need to run until parked
    assert_eq!(
        terminal_a.read_with(cx_a, |terminal, _| terminal.last_content.cells.clone()),
        terminal_b.read_with(cx_b, |terminal, _| terminal.last_content.cells.clone())
    );
}
