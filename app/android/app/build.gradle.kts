plugins {
    id("com.android.application")
    id("kotlin-android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "org.survivalpedia.survivalpedia"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = JavaVersion.VERSION_17.toString()
    }

    defaultConfig {
        applicationId = "com.pocketbothy.app"
        // Android 8+; arm64 only (the Rust core is built for aarch64-linux-android, see .github/workflows)
        minSdk = 26
        ndk { abiFilters += listOf("arm64-v8a") }
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    // A stable release key keeps app updates installable over each other (and keeps downloaded
    // content). CI decodes it from repo secrets; without them the debug key is used, which
    // differs per machine, so such builds can't update each other.
    val keystore = System.getenv("SP_KEYSTORE")?.let { file(it) }?.takeIf { it.exists() }
    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = keystore
                storePassword = System.getenv("SP_KEYSTORE_PASSWORD")
                keyAlias = "survivalpedia"
                keyPassword = System.getenv("SP_KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.getByName(if (keystore != null) "release" else "debug")
        }
    }
}

flutter {
    source = "../.."
}
