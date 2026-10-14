import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.api.DefaultTask
import org.gradle.api.file.ConfigurableFileCollection
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.tasks.InputFiles
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import javax.inject.Inject

plugins {
    id("com.android.application")
}

val protobufVersion = "4.36.2"
val protoRoot = rootProject.layout.projectDirectory.dir("../proto")

android {
    namespace = "io.github.hassanm0301.plasmaslate"
    compileSdk = 36

    defaultConfig {
        applicationId = "io.github.hassanm0301.plasmaslate"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // Signed with the debug key unless a release keystore is configured (see docs/RELEASING.md).
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    lint {
        abortOnError = true
        warningsAsErrors = false
        disable += setOf("GradleDependency", "NewerVersionAvailable", "AndroidGradlePluginVersion", "OldTargetApi")
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    jvmToolchain(21)
    compilerOptions {
        allWarningsAsErrors.set(false)
    }
}

/** Runs protoc (from Maven Central, no system install needed) to generate Java lite classes. */
abstract class ProtocTask : DefaultTask() {
    @get:InputFiles
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val protoFiles: ConfigurableFileCollection

    @get:InputFiles
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val protoc: ConfigurableFileCollection

    @get:org.gradle.api.tasks.Internal
    abstract val protoRootDir: DirectoryProperty

    @get:OutputDirectory
    abstract val outputDir: DirectoryProperty

    @get:Inject
    abstract val exec: ExecOperations

    @TaskAction
    fun generate() {
        val out = outputDir.get().asFile
        out.deleteRecursively()
        out.mkdirs()
        val exe = protoc.singleFile
        exe.setExecutable(true)
        val root = protoRootDir.get().asFile
        exec.exec {
            commandLine(
                listOf(exe.absolutePath, "--java_out=lite:${out.absolutePath}", "-I${root.absolutePath}") +
                    protoFiles.files.map { it.absolutePath }.sorted(),
            )
        }
    }
}

val protocExe: Configuration by configurations.creating

dependencies {
    protocExe("com.google.protobuf:protoc:$protobufVersion:linux-x86_64@exe")
    implementation("com.google.protobuf:protobuf-javalite:$protobufVersion")
    testImplementation("junit:junit:4.13.2")
}

val generateProto = tasks.register<ProtocTask>("generateProto") {
    protoFiles.from(fileTree(protoRoot) { include("**/*.proto") })
    protoc.from(protocExe)
    protoRootDir.set(protoRoot)
    outputDir.set(layout.buildDirectory.dir("generated/source/proto"))
}

extensions.getByType(ApplicationAndroidComponentsExtension::class.java).onVariants { variant ->
    variant.sources.java?.addGeneratedSourceDirectory(generateProto, ProtocTask::outputDir)
}
