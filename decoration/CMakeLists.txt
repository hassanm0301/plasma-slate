cmake_minimum_required(VERSION 3.22)
project(plasma-slate-decoration VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(ECM 6.0 REQUIRED NO_MODULE)
list(APPEND CMAKE_MODULE_PATH ${ECM_MODULE_PATH})
include(KDEInstallDirs6)
find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui DBus)
find_package(KF6CoreAddons REQUIRED)
find_package(KF6Config REQUIRED)
find_package(KDecoration3 6.7 REQUIRED)

add_library(plasmaslatedecoration MODULE
    decoration.cpp decoration.h layout.h windowmove.cpp windowmove.h)
target_link_libraries(plasmaslatedecoration PRIVATE
    Qt6::Core Qt6::Gui Qt6::DBus KF6::CoreAddons KF6::ConfigCore KDecoration3::KDecoration)
target_compile_options(plasmaslatedecoration PRIVATE -Wall -Wextra -Wpedantic)
set_target_properties(plasmaslatedecoration PROPERTIES
    PREFIX "" OUTPUT_NAME io.github.hassanm0301.plasmaslate)
qt_add_resources(plasmaslatedecoration windowmove PREFIX /plasma-slate FILES move-window.js)
set(DECORATION_INSTALL_DIR "${KDE_INSTALL_QTPLUGINDIR}/org.kde.kdecoration3"
    CACHE PATH "KWin decoration plugin installation directory")
install(TARGETS plasmaslatedecoration
    LIBRARY DESTINATION ${DECORATION_INSTALL_DIR})

option(PLASMA_SLATE_DECORATION_TESTS "Build the decoration's offscreen tests" OFF)
if(PLASMA_SLATE_DECORATION_TESTS)
    enable_testing()
    find_package(Qt6 REQUIRED COMPONENTS Test)
    add_executable(decoration-layout-test tests/layouttest.cpp)
    target_link_libraries(decoration-layout-test PRIVATE Qt6::Gui Qt6::Test)
    add_test(NAME decoration-layout COMMAND decoration-layout-test)
    add_executable(decoration-runtime-test tests/runtimetest.cpp)
    target_link_libraries(decoration-runtime-test PRIVATE Qt6::Gui Qt6::Test
        KF6::CoreAddons KF6::ConfigCore KDecoration3::KDecoration KDecoration3::KDecoration3Private)
    target_compile_definitions(decoration-runtime-test PRIVATE
        DECORATION_PLUGIN_PATH="$<TARGET_FILE:plasmaslatedecoration>")
    add_dependencies(decoration-runtime-test plasmaslatedecoration)
    add_test(NAME decoration-runtime COMMAND decoration-runtime-test)
    set_tests_properties(decoration-runtime PROPERTIES ENVIRONMENT "QT_QPA_PLATFORM=offscreen")
endif()
