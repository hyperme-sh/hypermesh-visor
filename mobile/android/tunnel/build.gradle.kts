plugins {
    id("com.android.library") version "8.9.1"
    id("org.jetbrains.kotlin.android") version "2.1.20"
}
android {
    namespace = "sh.hyperme.visor.mobile.tunnel"
    compileSdk = 36
    defaultConfig { minSdk = 28 }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    testOptions { unitTests.isReturnDefaultValues = true }
}
kotlin { jvmToolchain(17) }
dependencies {
    api("com.wireguard.android:tunnel:1.0.20260102")
    testImplementation("junit:junit:4.13.2")
}
