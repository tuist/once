set -eu
[ -f "$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/from-script.txt" ]
[ -x "$TARGET_BUILD_DIR/$EXECUTABLE_PATH" ]
printf 'run\n' >> "$TARGET_TEMP_DIR/runs.txt"
/usr/libexec/PlistBuddy -c 'Set :CFBundleVersion 42' "$TARGET_BUILD_DIR/$INFOPLIST_PATH"
wc -l < "$TARGET_TEMP_DIR/runs.txt" | tr -d ' ' > "$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/undeclared.txt"
rm "$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/generated-resource.txt"
