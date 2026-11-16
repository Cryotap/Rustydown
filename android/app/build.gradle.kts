plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "com.rustget.app"
    compileSdk = 34

    defaultConfig {
        applicationId = "com.rustget.app"
        minSdk = 26
        targetSdk = 34
        // Bump appVersionCode and versionName with every update so Android installs over the old app.
        // CI run numbers are also mixed in, so a rebuild is never older than the last install.
        val appVersionCode = 1
        val ciRun = System.getenv("GITHUB_RUN_NUMBER")?.toIntOrNull() ?: 0
        versionCode = maxOf(appVersionCode, ciRun + 100)
        versionName = "0.1"
        ndk { abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64") }
    }

    // CI always supplies a keystore (your secret, or the one committed in the repo).
    signingConfigs {
        create("ci") {
            val ks = System.getenv("KEYSTORE_FILE")
            if (ks != null && file(ks).exists()) {
                storeFile = file(ks)
                storePassword = System.getenv("KEYSTORE_PASSWORD")
                keyAlias = System.getenv("KEY_ALIAS")
                keyPassword = System.getenv("KEY_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("ci")
        }
    }

    // The Rust library is compiled by cargo-ndk into src/main/jniLibs.
    sourceSets["main"].jniLibs.srcDir("src/main/jniLibs")

    lint {
        abortOnError = false
        checkReleaseBuilds = false
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }
}
