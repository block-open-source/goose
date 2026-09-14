plugins {
    kotlin("jvm") version "2.2.21"
    `java-library`
    id("com.vanniktech.maven.publish") version "0.34.0"
}

group = "io.github.aaif-goose"
version = gooseSdkVersion()

fun gooseSdkVersion(): String {
    val cargoToml = file("../Cargo.toml").readText()
    return Regex("(?m)^version\\s*=\\s*\"([^\"]+)\"")
        .find(cargoToml)
        ?.groupValues
        ?.get(1)
        ?: error("Could not find goose-sdk version in ../Cargo.toml")
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_11)
    }
}

java {
    sourceCompatibility = JavaVersion.VERSION_11
    targetCompatibility = JavaVersion.VERSION_11
    withSourcesJar()
}

dependencies {
    api("net.java.dev.jna:jna:5.14.0")
    api("org.jetbrains.kotlinx:kotlinx-coroutines-core:1.10.2")
}

tasks.jar {
    manifest {
        attributes(
            "Implementation-Title" to "Goose GDK",
            "Implementation-Version" to project.version,
        )
    }
}

mavenPublishing {
    publishToMavenCentral(automaticRelease = true)
    if (providers.gradleProperty("signingInMemoryKey").isPresent) {
        signAllPublications()
    }

    coordinates(
        groupId = "io.github.aaif-goose",
        artifactId = "gdk",
        version = project.version.toString(),
    )

    pom {
        name.set("Goose GDK")
        description.set("Kotlin/JVM bindings for the goose Development Kit (GDK)")
        inceptionYear.set("2026")
        url.set("https://github.com/aaif-goose/goose")
        licenses {
            license {
                name.set("Apache License, Version 2.0")
                url.set("https://www.apache.org/licenses/LICENSE-2.0")
                distribution.set("repo")
            }
        }
        developers {
            developer {
                id.set("aaif")
                name.set("Agentic AI Foundation")
                email.set("ai-oss-tools@block.xyz")
            }
        }
        scm {
            connection.set("scm:git:https://github.com/aaif-goose/goose.git")
            developerConnection.set("scm:git:ssh://git@github.com/aaif-goose/goose.git")
            url.set("https://github.com/aaif-goose/goose")
        }
    }
}
