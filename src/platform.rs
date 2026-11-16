//! Thin Android bridge over JNI (clipboard, share-intent, keyboard, permissions, wake lock,
//! metered-network check). On desktop every function is a harmless stub.

#[cfg(target_os = "android")]
mod imp {
    use jni::objects::{GlobalRef, JObject, JString, JValue};
    use jni::{JNIEnv, JavaVM};
    use std::sync::Mutex;

    static WAKE_LOCK: Mutex<Option<GlobalRef>> = Mutex::new(None);

    /// Run `f` with a JNIEnv attached to this thread and a local-reference frame, so the
    /// long-lived render thread never leaks JNI local references.
    fn with_env<R>(f: impl FnOnce(&mut JNIEnv, &JObject) -> jni::errors::Result<R>) -> Option<R> {
        let ctx = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(ctx.vm().cast()) }.ok()?;
        let mut env = vm.attach_current_thread().ok()?;
        let activity = unsafe { JObject::from_raw(ctx.context().cast()) };
        let out = env.with_local_frame(48, |env| f(env, &activity));
        if env.exception_check().unwrap_or(false) {
            let _ = env.exception_clear();
            return None;
        }
        out.ok()
    }

    fn system_service<'a>(
        env: &mut JNIEnv<'a>,
        act: &JObject,
        name: &str,
    ) -> jni::errors::Result<JObject<'a>> {
        let n = env.new_string(name)?;
        env.call_method(
            act,
            "getSystemService",
            "(Ljava/lang/String;)Ljava/lang/Object;",
            &[JValue::Object(&n)],
        )?
        .l()
    }

    fn to_string(env: &mut JNIEnv, o: JObject) -> jni::errors::Result<String> {
        let s: String = env.get_string(&JString::from(o))?.into();
        Ok(s)
    }

    pub fn clipboard_text() -> Option<String> {
        with_env(|env, act| {
            let cm = system_service(env, act, "clipboard")?;
            if cm.is_null() {
                return Ok(None);
            }
            let clip = env
                .call_method(&cm, "getPrimaryClip", "()Landroid/content/ClipData;", &[])?
                .l()?;
            if clip.is_null() {
                return Ok(None);
            }
            if env.call_method(&clip, "getItemCount", "()I", &[])?.i()? < 1 {
                return Ok(None);
            }
            let item = env
                .call_method(
                    &clip,
                    "getItemAt",
                    "(I)Landroid/content/ClipData$Item;",
                    &[JValue::Int(0)],
                )?
                .l()?;
            let text = env
                .call_method(
                    &item,
                    "coerceToText",
                    "(Landroid/content/Context;)Ljava/lang/CharSequence;",
                    &[JValue::Object(act)],
                )?
                .l()?;
            if text.is_null() {
                return Ok(None);
            }
            let s = env
                .call_method(&text, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            Ok(Some(to_string(env, s)?))
        })
        .flatten()
    }

    /// URL (or shared text) this app was launched with, if any.
    pub fn launch_url() -> Option<String> {
        with_env(|env, act| {
            let intent = env
                .call_method(act, "getIntent", "()Landroid/content/Intent;", &[])?
                .l()?;
            if intent.is_null() {
                return Ok(None);
            }
            let data = env
                .call_method(&intent, "getDataString", "()Ljava/lang/String;", &[])?
                .l()?;
            if !data.is_null() {
                return Ok(Some(to_string(env, data)?));
            }
            let key = env.new_string("android.intent.extra.TEXT")?;
            let text = env
                .call_method(
                    &intent,
                    "getStringExtra",
                    "(Ljava/lang/String;)Ljava/lang/String;",
                    &[JValue::Object(&key)],
                )?
                .l()?;
            if text.is_null() {
                return Ok(None);
            }
            Ok(Some(to_string(env, text)?))
        })
        .flatten()
    }

    pub fn show_keyboard() {
        let _ = with_env(|env, act| {
            let imm = system_service(env, act, "input_method")?;
            if !imm.is_null() {
                // SHOW_FORCED = 2
                env.call_method(
                    &imm,
                    "toggleSoftInput",
                    "(II)V",
                    &[JValue::Int(2), JValue::Int(0)],
                )?;
            }
            Ok(())
        });
    }

    pub fn has_all_files_access() -> bool {
        with_env(|env, _| {
            let sdk = env
                .get_static_field("android/os/Build$VERSION", "SDK_INT", "I")?
                .i()?;
            if sdk < 30 {
                return Ok(false);
            }
            env.call_static_method(
                "android/os/Environment",
                "isExternalStorageManager",
                "()Z",
                &[],
            )?
            .z()
        })
        .unwrap_or(false)
    }

    pub fn request_all_files_access() {
        let _ = with_env(|env, act| {
            let pkg_obj = env
                .call_method(act, "getPackageName", "()Ljava/lang/String;", &[])?
                .l()?;
            let pkg = to_string(env, pkg_obj)?;
            let uri_s = env.new_string(format!("package:{pkg}"))?;
            let uri = env
                .call_static_method(
                    "android/net/Uri",
                    "parse",
                    "(Ljava/lang/String;)Landroid/net/Uri;",
                    &[JValue::Object(&uri_s)],
                )?
                .l()?;
            let action =
                env.new_string("android.settings.MANAGE_APP_ALL_FILES_ACCESS_PERMISSION")?;
            let intent = env.new_object(
                "android/content/Intent",
                "(Ljava/lang/String;Landroid/net/Uri;)V",
                &[JValue::Object(&action), JValue::Object(&uri)],
            )?;
            env.call_method(
                act,
                "startActivity",
                "(Landroid/content/Intent;)V",
                &[JValue::Object(&intent)],
            )?;
            Ok(())
        });
    }

    pub fn is_metered() -> bool {
        with_env(|env, act| {
            let cm = system_service(env, act, "connectivity")?;
            if cm.is_null() {
                return Ok(false);
            }
            env.call_method(&cm, "isActiveNetworkMetered", "()Z", &[])?
                .z()
        })
        .unwrap_or(false)
    }

    /// Keep the CPU awake while downloads run.
    pub fn set_wake_lock(on: bool) {
        let mut slot = WAKE_LOCK.lock().unwrap();
        if on == slot.is_some() {
            return;
        }
        if on {
            let lock = with_env(|env, act| {
                let pm = system_service(env, act, "power")?;
                if pm.is_null() {
                    return Ok(None);
                }
                let tag = env.new_string("rustget:downloads")?;
                // PARTIAL_WAKE_LOCK = 1
                let wl = env
                    .call_method(
                        &pm,
                        "newWakeLock",
                        "(ILjava/lang/String;)Landroid/os/PowerManager$WakeLock;",
                        &[JValue::Int(1), JValue::Object(&tag)],
                    )?
                    .l()?;
                env.call_method(&wl, "acquire", "()V", &[])?;
                Ok(Some(env.new_global_ref(&wl)?))
            })
            .flatten();
            *slot = lock;
        } else if let Some(lock) = slot.take() {
            let _ = with_env(|env, _| {
                env.call_method(lock.as_obj(), "release", "()V", &[])?;
                Ok(())
            });
        }
    }
}

#[cfg(not(target_os = "android"))]
mod imp {
    pub fn clipboard_text() -> Option<String> {
        None
    }
    pub fn launch_url() -> Option<String> {
        None
    }
    pub fn show_keyboard() {}
    pub fn has_all_files_access() -> bool {
        true
    }
    pub fn request_all_files_access() {}
    pub fn is_metered() -> bool {
        false
    }
    pub fn set_wake_lock(_on: bool) {}
}

pub use imp::*;
