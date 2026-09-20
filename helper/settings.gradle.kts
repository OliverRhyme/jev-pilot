pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}
dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "jev-pilot-helper"

// One codebase, two packages. Everything that reads or serves a screen lives
// in :shared; the two apps are entry points over it.
//
// They are separate packages because they have to be. An instrumentation runs
// in its target package's process, so stopping the reader kills whatever
// shares that package — and when that was the accessibility service, every
// stop left it enabled, unbound, and never rebound by Android.
include(":shared", ":service", ":reader")
