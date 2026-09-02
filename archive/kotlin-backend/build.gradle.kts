plugins {
    kotlin("jvm") version "2.1.0"
    kotlin("plugin.serialization") version "2.1.0"
    application
}

group = "com.hdrconverter"
version = "1.0.0"

repositories {
    maven { url = uri("https://maven.aliyun.com/repository/public") }
    maven { url = uri("https://maven.aliyun.com/repository/google") }
    mavenCentral()
}

val ktorVersion = "3.0.3"

dependencies {
    // Ktor HTTP 服务
    implementation("io.ktor:ktor-server-core:$ktorVersion")
    implementation("io.ktor:ktor-server-netty:$ktorVersion")
    implementation("io.ktor:ktor-server-content-negotiation:$ktorVersion")
    implementation("io.ktor:ktor-serialization-kotlinx-json:$ktorVersion")
    implementation("io.ktor:ktor-server-status-pages:$ktorVersion")

    // 序列化
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.7.3")

    // 日志
    implementation("ch.qos.logback:logback-classic:1.5.15")
}

application {
    mainClass.set("com.hdrconverter.MainKt")
}

tasks.jar {
    archiveBaseName.set("hdr-converter-backend")
    archiveVersion.set("")
    manifest {
        attributes["Main-Class"] = "com.hdrconverter.MainKt"
    }
    // 打包所有依赖为 fat JAR
    from(configurations.runtimeClasspath.get().map { if (it.isDirectory) it else zipTree(it) })
    duplicatesStrategy = DuplicatesStrategy.EXCLUDE
}
