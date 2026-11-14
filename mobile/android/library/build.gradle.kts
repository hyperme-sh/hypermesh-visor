import org.gradle.api.tasks.bundling.AbstractArchiveTask

plugins {
    id("com.android.library") version "8.9.1"
    id("org.jetbrains.kotlin.android") version "2.1.20"
}

val bindings = providers.environmentVariable("VISOR_MOBILE_BINDINGS").get()
val nativeLibrary = providers.environmentVariable("VISOR_MOBILE_ANDROID_LIB").get()
val stageSources by tasks.registering(Copy::class) {
    from(file("../AndroidSessionStore.kt"), file("../EncryptedSessionStore.kt"))
    from(file("$bindings/uniffi/hypermesh_visor_mobile/hypermesh_visor_mobile.kt"))
    into(layout.buildDirectory.dir("generated/visor-sources"))
}
val stageNative by tasks.registering(Copy::class) {
    from(file(nativeLibrary))
    into(layout.buildDirectory.dir("generated/visor-jni/arm64-v8a"))
}

android {
    namespace = "sh.hyperme.visor.mobile"
    compileSdk = 35
    defaultConfig {
        minSdk = 28
        consumerProguardFiles("consumer-rules.pro")
    }
    sourceSets["main"].java.srcDir(layout.buildDirectory.dir("generated/visor-sources"))
    sourceSets["main"].jniLibs.srcDir(layout.buildDirectory.dir("generated/visor-jni"))
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildTypes { getByName("release") { isMinifyEnabled = false } }
    packaging { jniLibs.keepDebugSymbols += "**/libhypermesh_visor_mobile.so" }
}
kotlin { jvmToolchain(17) }
dependencies { api("net.java.dev.jna:jna:5.17.0@aar") }
tasks.named("preBuild") { dependsOn(stageSources, stageNative) }
tasks.withType<AbstractArchiveTask>().configureEach {
    isPreserveFileTimestamps = false
    isReproducibleFileOrder = true
}
