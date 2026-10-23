import java.util.Properties
import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("rust")
}

val tauriProperties = Properties().apply {
    val propFile = file("tauri.properties")
    if (propFile.exists()) {
        propFile.inputStream().use { load(it) }
    }
}

// Release signing with your own key (apps/desktop/scripts/android-release.sh): the keystore's
// path, alias and passwords come from the environment, never from a file in the repo. Without
// FILEBRIDGE_ANDROID_KEYSTORE the release APK is signed with the debug key, which is fine for
// trying it but can't update an install signed with your own key (and the other way around).
fun env(name: String): String? = providers.environmentVariable(name).orNull?.takeIf { it.isNotEmpty() }
val releaseKeystore = env("FILEBRIDGE_ANDROID_KEYSTORE")

android {
    compileSdk = 37
    namespace = "app.filebridge"
    defaultConfig {
        manifestPlaceholders["usesCleartextTraffic"] = "false"
        applicationId = "app.filebridge"
        minSdk = 26
        targetSdk = 37
        versionCode = tauriProperties.getProperty("tauri.android.versionCode", "1").toInt()
        versionName = tauriProperties.getProperty("tauri.android.versionName", "1.0")
    }
    signingConfigs {
        if (releaseKeystore != null) {
            create("release") {
                fun required(name: String) = env(name)
                    ?: throw GradleException("FILEBRIDGE_ANDROID_KEYSTORE is set but $name is not")
                storeFile = file(releaseKeystore)
                if (!storeFile!!.isFile) {
                    throw GradleException("FILEBRIDGE_ANDROID_KEYSTORE: no file at $releaseKeystore")
                }
                storePassword = required("FILEBRIDGE_ANDROID_KEYSTORE_PASSWORD")
                keyAlias = required("FILEBRIDGE_ANDROID_KEY_ALIAS")
                // A PKCS12 keystore (keytool's default) has one password for the store and the key.
                keyPassword = env("FILEBRIDGE_ANDROID_KEY_PASSWORD") ?: storePassword
            }
        }
    }
    buildTypes {
        getByName("debug") {
            manifestPlaceholders["usesCleartextTraffic"] = "true"
            isDebuggable = true
            isJniDebuggable = true
            isMinifyEnabled = false
            packaging {
                jniLibs.keepDebugSymbols.add("*/arm64-v8a/*.so")
                jniLibs.keepDebugSymbols.add("*/armeabi-v7a/*.so")
                jniLibs.keepDebugSymbols.add("*/x86/*.so")
                jniLibs.keepDebugSymbols.add("*/x86_64/*.so")
            }
        }
        getByName("release") {
            signingConfig = signingConfigs.findByName("release") ?: signingConfigs.getByName("debug")
            optimization {
               enable = true
            }
            proguardFiles(
                *fileTree(".") {
                  include("**/*.pro")
                  exclude("build/**")
                }.files.toTypedArray()
            )
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_1_8
        targetCompatibility = JavaVersion.VERSION_1_8
    }
    buildFeatures {
        buildConfig = true
    }
}

kotlin {
    compilerOptions {
        jvmTarget = JvmTarget.JVM_1_8
    }
}

rust {
    rootDirRel = "../../../"
}

dependencies {
    implementation("androidx.webkit:webkit:1.14.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("androidx.activity:activity-ktx:1.10.1")
    implementation("com.google.android.material:material:1.12.0")
    implementation("androidx.lifecycle:lifecycle-process:2.10.0")
    testImplementation("junit:junit:4.13.2")
    androidTestImplementation("androidx.test.ext:junit:1.1.4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.5.0")
}

apply(from = file("tauri.build.gradle.kts"))
