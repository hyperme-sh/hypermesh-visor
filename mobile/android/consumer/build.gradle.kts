plugins {
    id("com.android.application") version "8.9.1"
    id("org.jetbrains.kotlin.android") version "2.1.20"
}

android {
    namespace = "sh.hyperme.visor.mobile.contract"
    compileSdk = 35
    buildToolsVersion = "35.0.0"
    defaultConfig {
        applicationId = "sh.hyperme.visor.mobile.contract"
        minSdk = 28
        targetSdk = 35
        ndk { abiFilters += "arm64-v8a" }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildTypes { getByName("release") { isMinifyEnabled = false } }
    packaging {
        jniLibs.useLegacyPackaging = false
        jniLibs.keepDebugSymbols += "**/*.so"
    }
}
kotlin { jvmToolchain(17) }
dependencies {
    implementation(files(providers.environmentVariable("VISOR_MOBILE_AAR").get()))
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    implementation("org.jetbrains.kotlin:kotlin-stdlib:2.1.20")
}
