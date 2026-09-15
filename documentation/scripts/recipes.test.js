const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const test = require("node:test");
const vm = require("node:vm");
const ts = require("typescript");

function loadTypeScriptModule(relativePath, configureRequire = () => {}) {
  const sourcePath = path.join(__dirname, "..", relativePath);
  const source = fs.readFileSync(sourcePath, "utf8");
  const javascript = ts.transpileModule(source, {
    compilerOptions: {
      module: ts.ModuleKind.CommonJS,
      target: ts.ScriptTarget.ES2020,
    },
  }).outputText;
  const moduleObject = { exports: {} };
  const fakeRequire = (id) => {
    throw new Error(`unexpected runtime require: ${id}`);
  };

  configureRequire(fakeRequire);
  vm.runInNewContext(javascript, {
    module: moduleObject,
    exports: moduleObject.exports,
    require: fakeRequire,
    Buffer,
  });
  return moduleObject.exports;
}

function loadRecipesModule(recipeMap) {
  return loadTypeScriptModule("src/utils/recipes.ts", (fakeRequire) => {
    fakeRequire.context = () => {
      const context = (key) => recipeMap[key];
      context.keys = () => Object.keys(recipeMap);
      return context;
    };
  });
}

test("preserves the discovered path for a .yml-only recipe", async () => {
  const recipes = loadRecipesModule({
    "./reviewed.yml": {
      default: {
        title: "Reviewed recipe",
        description: "Uses the discovered source path",
      },
    },
  });

  const [recipe] = await recipes.searchRecipes("");

  assert.equal(recipe.id, "reviewed");
  assert.equal(
    recipe.localPath,
    "documentation/src/pages/recipes/data/recipes/reviewed.yml",
  );
});

test("preserves the normal .yaml recipe path", () => {
  const recipes = loadRecipesModule({
    "./standard.yaml": {
      default: {
        title: "Standard recipe",
        description: "Uses the standard extension",
      },
    },
  });

  assert.equal(
    recipes.getRecipeById("standard").localPath,
    "documentation/src/pages/recipes/data/recipes/standard.yaml",
  );
});

test("rejects same-stem .yml and .yaml recipes", () => {
  assert.throws(
    () =>
      loadRecipesModule({
        "./ambiguous.yml": {
          default: { title: "First recipe", description: "First source" },
        },
        "./ambiguous.yaml": {
          default: { title: "Second recipe", description: "Second source" },
        },
      }),
    /Ambiguous recipe ID "ambiguous" from "ambiguous\.yml" and "ambiguous\.yaml"/,
  );
});

test("card and detail command variants use the supplied source path", () => {
  const { buildRecipeCliCommand } = loadTypeScriptModule(
    "src/utils/recipe-command.ts",
  );
  const localPath = "documentation/src/pages/recipes/data/recipes/reviewed.yml";

  assert.equal(
    buildRecipeCliCommand(localPath),
    `goose run --recipe ${localPath}`,
  );
  assert.equal(
    buildRecipeCliCommand(localPath, "topic=security"),
    `goose run --recipe ${localPath} --params topic=security`,
  );
});
