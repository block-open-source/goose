export function buildRecipeCliCommand(
  localPath: string,
  filledParams = "",
): string {
  return `goose run --recipe ${localPath}${filledParams ? ` --params ${filledParams}` : ""}`;
}
