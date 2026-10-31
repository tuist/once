set -eu
[ "$ONCE_SCRIPT_CI_VALUE" = 'from caller' ]
[ "$DWARF_DSYM_FOLDER_PATH" = "$TARGET_BUILD_DIR" ]
[ "$DWARF_DSYM_FILE_NAME" = "$FULL_PRODUCT_NAME.dSYM" ]
[ "$SCRIPT_INPUT_FILE_COUNT" = 1 ]
mkdir -p "$SCRIPT_OUTPUT_FILE_0"
printf 'unrelated\n' > "$SCRIPT_OUTPUT_FILE_0/value.txt"
cat "$SCRIPT_INPUT_FILE_0" > "$SCRIPT_OUTPUT_FILE_1"
printf 'run\n' >> "$TARGET_TEMP_DIR/preparation-runs.txt"
