//! Minimal repro: an inner `<Suspense>` whose body reads a `LocalResource`,
//! constructed inside the resolved value of an outer `Suspend`. The id the
//! server registers via `set_incomplete_chunk` drifts from the id the client
//! allocates at hydrate time, because the outer body closure is re-invoked
//! across `dry_resolve`, the Effect double-check pass, and the final
//! `resolve` — each invocation constructing a fresh inner `<Suspense>` and
//! consuming a new `SerializedDataId`. The hydrate-time consequence is
//! `failed_to_cast_marker_node`.
//!
//! This binary observes the bug on the SSR side: the only signal the server
//! sends to the client telling it to start in the fallback branch is the
//! `set_incomplete_chunk(self.id)` registration in
//! `leptos/src/suspense_component.rs`, keyed by the drift-prone
//! `SerializedDataId`. Carrying that signal in the DOM stream (e.g. as a
//! comment marker emitted alongside the fallback HTML) would let the
//! client's hydrate path detect the local-resource case without depending
//! on id parity across server closure re-invocations.

use any_spawner::Executor;
use futures::StreamExt;
use hydration_context::SsrSharedContext;
use leptos::prelude::*;
use std::sync::Arc;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    _ = Executor::init_tokio();
    let owner = Owner::new_root(Some(Arc::new(SsrSharedContext::new())));
    owner.set();

    // Outer (non-local) resource so the outer body resolves asynchronously,
    // exercising the closure-reinvocation path inside
    // `SuspenseBoundary::to_html_async_with_buf`.
    let outer = Resource::new(
        || (),
        |_| async {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        },
    );

    // LocalResource read inside the inner Suspense's body. On SSR this is
    // forever pending; reading it fires `LocalResourceNotifier::notify()`
    // and forces the inner Suspense down the `local_rx` arm of
    // `to_html_async_with_buf`'s `select!`.
    let local: LocalResource<()> = LocalResource::new(|| async move {});

    let app = view! {
        <Suspense fallback=|| view! { <p>"outer loading"</p> }>
            {move || {
                let local = local.clone();
                Suspend::new(async move {
                    outer.await;
                    let local = local.clone();
                    view! {
                        <Suspense fallback=|| view! { <p>"inner loading"</p> }>
                            {move || {
                                let local = local.clone();
                                Suspend::new(async move {
                                    local.await;
                                    view! { <span>"done"</span> }
                                })
                            }}
                        </Suspense>
                    }
                })
            }}
        </Suspense>
    };

    let html = app.to_html_stream_out_of_order().collect::<String>().await;
    let has_dom_signal = html.contains("<!--s-L-->");

    println!("SSR HTML (length {}):\n{html}\n", html.len());
    println!("DOM-side local-fallback signal present: {has_dom_signal}");
    println!();

    if has_dom_signal {
        println!(
            "OK — a DOM-side signal accompanies the inner Suspense's\n\
             fallback. A client hydrate path that detects this signal can\n\
             set `starts_local` without depending on the drift-prone\n\
             `__INCOMPLETE_CHUNKS` id lookup."
        );
    } else {
        println!(
            "BUG REPRODUCED — the server emitted the inner Suspense's\n\
             fallback with no DOM-side signal. The only channel that tells\n\
             the client this Suspense should start in fallback is the\n\
             `set_incomplete_chunk(self.id)` registration. Because the inner\n\
             Suspense is constructed multiple times during one SSR pass\n\
             (across the outer closure's `dry_resolve`, double-check, and\n\
             `resolve` re-invocations), the registered id is from the LAST\n\
             construction — but the client's single hydrate pass allocates\n\
             the FIRST id at the same call site. The lookup misses, the\n\
             client picks the children branch while the server emitted the\n\
             fallback branch, and hydration walks for the wrong marker —\n\
             panics in `failed_to_cast_marker_node`."
        );
        std::process::exit(1);
    }
}
