cmake_minimum_required(VERSION 3.20)
project(OnceCMakeExample LANGUAGES C)

add_library(greeting STATIC src/greeting.c)
target_include_directories(greeting PUBLIC include)

enable_testing()
add_test(NAME greeting-configured COMMAND "${CMAKE_COMMAND}" -E echo "greeting configured")
