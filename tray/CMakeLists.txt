cmake_minimum_required(VERSION 3.22)
project(plasma-slate-tray VERSION 0.1.0 LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_AUTOMOC ON)

find_package(Qt6 6.5 REQUIRED COMPONENTS Core Gui Widgets DBus)
find_package(KF6StatusNotifierItem REQUIRED)

add_executable(plasma-slate-tray src/main.cpp src/tray.cpp src/tray.h)
target_link_libraries(plasma-slate-tray PRIVATE Qt6::Widgets Qt6::DBus KF6::StatusNotifierItem)
target_compile_options(plasma-slate-tray PRIVATE -Wall -Wextra -Werror)

include(GNUInstallDirs)
install(TARGETS plasma-slate-tray RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
