cmake_minimum_required(VERSION 3.22)
project(plasma-slate-tray VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Widgets DBus Qml Quick)
find_package(KF6StatusNotifierItem REQUIRED)
find_package(KF6WindowSystem REQUIRED)
qt_standard_project_setup(REQUIRES 6.5)
qt_policy(SET QTP0004 NEW)

qt_add_executable(plasma-slate-tray
    src/main.cpp
    src/app.cpp src/app.h
    src/slateclient.cpp src/slateclient.h
    src/tray.cpp src/tray.h)
# The window. Kirigami and Kirigami Addons are loaded at runtime from the system, so the
# import scan (only needed for static Qt builds) is skipped.
qt_add_qml_module(plasma-slate-tray
    URI io.github.hassanm0301.plasmaslate
    NO_IMPORT_SCAN
    QML_FILES qml/Main.qml)
target_link_libraries(plasma-slate-tray PRIVATE
    Qt6::Widgets Qt6::DBus Qt6::Qml Qt6::Quick KF6::StatusNotifierItem KF6::WindowSystem)
target_compile_options(plasma-slate-tray PRIVATE -Wall -Wextra -Werror)

include(GNUInstallDirs)
install(TARGETS plasma-slate-tray RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
