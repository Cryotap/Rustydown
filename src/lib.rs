//! RustGet core: the download engine, exposed to the Kotlin UI over JNI (librustget.so).

mod engine;
mod logic;
mod net;

use engine::{Manager, NewDownload, Settings};
use jni::objects::{JClass, JString};
use jni::sys::{jboolean, jint, jlong, jstring, JNI_TRUE};
use jni::JNIEnv;
use std::path::PathBuf;
use std::sync::OnceLock;

static MGR: OnceLock<Manager> = OnceLock::new();

fn mgr() -> &'static Manager {
    MGR.get().expect("Native.init was not called")
}

fn get(env: &mut JNIEnv, s: &JString) -> String {
    env.get_string(s).map(String::from).unwrap_or_default()
}

fn ret(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s)
        .map(|j| j.into_raw())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_init(
    mut env: JNIEnv,
    _c: JClass,
    data_dir: JString,
    default_dir: JString,
) {
    let data = get(&mut env, &data_dir);
    let dir = get(&mut env, &default_dir);
    MGR.get_or_init(|| Manager::new(PathBuf::from(data), dir));
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_snapshot(
    mut env: JNIEnv,
    _c: JClass,
) -> jstring {
    let json = serde_json::to_string(&mgr().snapshot()).unwrap_or_else(|_| "{}".into());
    ret(&mut env, &json)
}

/// JSON NewDownload → "ok:<id>" or "error:<message>".
#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_add(
    mut env: JNIEnv,
    _c: JClass,
    json: JString,
) -> jstring {
    let text = get(&mut env, &json);
    let out = match serde_json::from_str::<NewDownload>(&text) {
        Ok(n) => match mgr().add(n) {
            Ok(id) => format!("ok:{id}"),
            Err(e) => format!("error:{e}"),
        },
        Err(e) => format!("error:bad request: {e}"),
    };
    ret(&mut env, &out)
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_pause(_env: JNIEnv, _c: JClass, id: jlong) {
    mgr().pause(id as u64);
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_resume(_env: JNIEnv, _c: JClass, id: jlong) {
    mgr().resume(id as u64);
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_remove(
    _env: JNIEnv,
    _c: JClass,
    id: jlong,
    delete_file: jboolean,
) {
    mgr().remove(id as u64, delete_file == JNI_TRUE);
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_pauseAll(_env: JNIEnv, _c: JClass) {
    mgr().pause_all();
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_resumeAll(_env: JNIEnv, _c: JClass) {
    mgr().resume_all();
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_clearFinished(_env: JNIEnv, _c: JClass) {
    mgr().clear_finished();
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_setLimit(
    _env: JNIEnv,
    _c: JClass,
    id: jlong,
    kbps: jint,
) {
    mgr().set_limit(id as u64, kbps.max(0) as u32);
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_getSettings(
    mut env: JNIEnv,
    _c: JClass,
) -> jstring {
    let json = serde_json::to_string(&mgr().settings()).unwrap_or_else(|_| "{}".into());
    ret(&mut env, &json)
}

/// Returns "" on success, else an error message.
#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_setSettings(
    mut env: JNIEnv,
    _c: JClass,
    json: JString,
) -> jstring {
    let text = get(&mut env, &json);
    let out = match serde_json::from_str::<Settings>(&text) {
        Ok(s) => {
            mgr().set_settings(s);
            String::new()
        }
        Err(e) => format!("Bad settings: {e}"),
    };
    ret(&mut env, &out)
}

#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_setMetered(
    _env: JNIEnv,
    _c: JClass,
    metered: jboolean,
) {
    mgr().set_metered(metered == JNI_TRUE);
}

/// Text box contents → JSON array of jobs, each [primary, mirrors…].
#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_parseLines(
    mut env: JNIEnv,
    _c: JClass,
    text: JString,
) -> jstring {
    let t = get(&mut env, &text);
    let json = serde_json::to_string(&logic::parse_lines(&t)).unwrap_or_else(|_| "[]".into());
    ret(&mut env, &json)
}

/// Any text → JSON array of the http(s) links inside it.
#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_urlsIn(
    mut env: JNIEnv,
    _c: JClass,
    text: JString,
) -> jstring {
    let t = get(&mut env, &text);
    let json = serde_json::to_string(&logic::urls_in(&t)).unwrap_or_else(|_| "[]".into());
    ret(&mut env, &json)
}

/// Blocking: fetch a web page and list its links. Call from a background thread.
/// Returns {"links":[…]} or {"error":"…"}.
#[no_mangle]
pub extern "system" fn Java_com_rustget_app_Native_fetchLinks(
    mut env: JNIEnv,
    _c: JClass,
    page: JString,
    user_agent: JString,
) -> jstring {
    let page = get(&mut env, &page);
    let ua = get(&mut env, &user_agent);
    let agent = net::agent(&ua);
    let out = match net::fetch_page(&agent, page.trim()) {
        Ok((final_url, html)) => {
            let mut links: Vec<String> = Vec::new();
            for raw in logic::extract_links(&html) {
                if let Some(u) = net::resolve(&final_url, &raw) {
                    if !links.contains(&u) {
                        links.push(u);
                    }
                }
            }
            serde_json::json!({ "links": links })
        }
        Err(e) => serde_json::json!({ "error": e }),
    };
    ret(&mut env, &out.to_string())
}
