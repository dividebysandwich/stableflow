use leptos::ev;
use leptos::prelude::*;

use crate::api::{admin_create_user, admin_delete_user, admin_set_password, list_users};
use crate::app::CurrentUserCtx;
use crate::components::{alert, confirm};

/// Native browser prompt. Returns the entered string (trimmed of the empty case)
/// or `None` if cancelled. No-op off the wasm client.
fn prompt(message: &str) -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        leptos::prelude::window()
            .prompt_with_message(message)
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = message;
        None
    }
}

/// Admin-only page: list users, add new ones, reset passwords, delete accounts.
#[component]
pub fn AdminPage() -> impl IntoView {
    let me = expect_context::<CurrentUserCtx>().0;
    let my_name = move || me.get().flatten().map(|u| u.username).unwrap_or_default();

    let create = Action::new(|input: &(String, String, bool)| {
        let (u, p, a) = input.clone();
        async move { admin_create_user(u, p, a).await }
    });
    let delete = Action::new(|id: &i64| {
        let id = *id;
        async move { admin_delete_user(id).await }
    });
    let setpw = Action::new(|input: &(i64, String)| {
        let (id, p) = input.clone();
        async move { admin_set_password(id, p).await }
    });

    // Refetch the user list whenever any mutation completes.
    let users = Resource::new(
        move || {
            (
                create.version().get(),
                delete.version().get(),
                setpw.version().get(),
            )
        },
        |_| async move { list_users().await },
    );

    // Surface server errors (e.g. duplicate username, last-admin guard) as alerts.
    let report = |res: Option<Result<(), ServerFnError>>| {
        if let Some(Err(e)) = res {
            alert(&e.to_string());
        }
    };
    Effect::new(move |_| report(create.value().get()));
    Effect::new(move |_| report(delete.value().get()));
    Effect::new(move |_| {
        if let Some(Ok(())) = setpw.value().get() {
            alert("Password updated.");
        } else {
            report(setpw.value().get());
        }
    });

    // Add-user form state.
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let make_admin = RwSignal::new(false);

    let submit = move |ev: ev::SubmitEvent| {
        ev.prevent_default();
        let u = username.get();
        let p = password.get();
        if u.trim().is_empty() || p.is_empty() {
            alert("Username and password are required.");
            return;
        }
        create.dispatch((u, p, make_admin.get()));
        username.set(String::new());
        password.set(String::new());
        make_admin.set(false);
    };

    view! {
        <div class="page">
            <h1>"User administration"</h1>

            <Transition fallback=|| view! { <p class="muted">"Loading\u{2026}"</p> }>
                {move || {
                    match users.get() {
                        Some(Ok(list)) => {
                            let my_name = my_name();
                            view! {
                                <table class="users">
                                    <thead>
                                        <tr>
                                            <th>"User"</th>
                                            <th>"Role"</th>
                                            <th>"Jobs"</th>
                                            <th>"Created"</th>
                                            <th>"Actions"</th>
                                        </tr>
                                    </thead>
                                    <tbody>
                                        <For
                                            each=move || list.clone()
                                            key=|u| u.id
                                            children=move |u| {
                                                let uid = u.id;
                                                let uname = u.username.clone();
                                                let is_me = uname == my_name;
                                                let reset_name = uname.clone();
                                                let del_name = uname.clone();
                                                let reset = move |_| {
                                                    if let Some(p) = prompt(
                                                        &format!("New password for {reset_name}:")
                                                    ) {
                                                        setpw.dispatch((uid, p));
                                                    }
                                                };
                                                let del = move |_| {
                                                    if confirm(&format!(
                                                        "Delete user {del_name} and all their jobs/images? This cannot be undone."
                                                    )) {
                                                        delete.dispatch(uid);
                                                    }
                                                };
                                                view! {
                                                    <tr>
                                                        <td>
                                                            {uname}
                                                            {is_me.then(|| view! { <span class="muted">" (you)"</span> })}
                                                        </td>
                                                        <td>
                                                            {if u.is_admin {
                                                                view! { <span class="badge badge-admin">"admin"</span> }.into_any()
                                                            } else {
                                                                view! { <span class="muted">"user"</span> }.into_any()
                                                            }}
                                                        </td>
                                                        <td>{u.job_count}</td>
                                                        <td class="muted">{u.created_at.clone()}</td>
                                                        <td class="user-actions">
                                                            <button class="link-btn" on:click=reset>"Reset password"</button>
                                                            {(!is_me).then(move || view! {
                                                                <button class="link-btn danger" on:click=del>"Delete"</button>
                                                            })}
                                                        </td>
                                                    </tr>
                                                }
                                            }
                                        />
                                    </tbody>
                                </table>
                            }
                                .into_any()
                        }
                        Some(Err(_)) => view! {
                            <p class="error">"Access denied \u{2014} admin only."</p>
                        }
                            .into_any(),
                        None => view! { <p class="muted">"Loading\u{2026}"</p> }.into_any(),
                    }
                }}
            </Transition>

            <h2>"Add user"</h2>
            <form class="add-user" on:submit=submit>
                <input
                    type="text"
                    placeholder="Username"
                    autocomplete="off"
                    prop:value=move || username.get()
                    on:input=move |e| username.set(event_target_value(&e))
                />
                <input
                    type="password"
                    placeholder="Password"
                    autocomplete="new-password"
                    prop:value=move || password.get()
                    on:input=move |e| password.set(event_target_value(&e))
                />
                <label class="admin-check">
                    <input
                        type="checkbox"
                        prop:checked=move || make_admin.get()
                        on:change=move |e| make_admin.set(event_target_checked(&e))
                    />
                    "Administrator"
                </label>
                <button type="submit">"Create user"</button>
            </form>
        </div>
    }
}
