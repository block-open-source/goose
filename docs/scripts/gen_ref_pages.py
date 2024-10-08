from pathlib import Path

import mkdocs_gen_files

nav = mkdocs_gen_files.Nav()

root = Path(__file__).parent.parent.parent
src = root / "src"

# Collecting all modules for the index page
module_links = []
core_modules = []
toolkit_modules = []
utils_modules = []

for path in sorted(src.rglob("*.py")):
    module_path = path.relative_to(src).with_suffix("")  # Removes the '.py' suffix
    doc_path = path.relative_to(src).with_suffix(".md")  # Creates .md path

    full_doc_path = Path("reference", doc_path)

    parts = tuple(module_path.parts)

    if parts[-1] == "__init__":
        parts = parts[:-1]
        doc_path = doc_path.with_name("index.md")
        full_doc_path = full_doc_path.with_name("index.md")
    elif parts[-1] == "__main__":
        continue

    # Construct a dynamic identifier based on module path
    ident = ".".join(parts)

    # Organize modules into categories
    if "toolkit" in ident:
        toolkit_modules.append(f"- [{ident}]({doc_path})")
    elif "utils" in ident:
        utils_modules.append(f"- [{ident}]({doc_path})")
    else:
        core_modules.append(f"- [{ident}]({doc_path})")

    # Generate the markdown file for each module
    with mkdocs_gen_files.open(full_doc_path, "w") as fd:
        fd.write(f"::: {ident}")

    nav[parts] = doc_path.as_posix()

# Write the enhanced SUMMARY.md with categories
with mkdocs_gen_files.open("reference/SUMMARY.md", "w") as nav_file:
    nav_file.write("# Reference Documentation Summary\n\n")

    nav_file.write("## Core Modules\n")
    nav_file.write("\n".join(core_modules) + "\n\n")

    nav_file.write("## Toolkit Modules\n")
    nav_file.write("\n".join(toolkit_modules) + "\n\n")

    nav_file.write("## Utility Modules\n")
    nav_file.write("\n".join(utils_modules) + "\n\n")

# Create an index.md file for the reference section
with mkdocs_gen_files.open("reference/index.md", "w") as index_file:
    index_file.write("# Reference Documentation\n\n")
    index_file.write("Welcome to the reference documentation for the project's Python modules.\n\n")
    index_file.write("Below is a list of available modules:\n\n")
    index_file.write("\n".join(core_modules + toolkit_modules + utils_modules))


