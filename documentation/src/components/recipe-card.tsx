// Full updated RecipeCard.tsx
import React, { useState } from "react";
import toast from "react-hot-toast";
import Link from "@docusaurus/Link";
import { buildRecipeCliCommand } from "@site/src/utils/recipe-command";

export type Recipe = {
  id: string;
  title: string;
  description: string;
  extensions: string[];
  activities: string[];
  recipeUrl: string;
  localPath: string;
  action?: string;
  author?: {
    contact?: string;
  };
  persona?: string;
  parameters?: { key: string; requirement: string; value?: string }[];
};

export function RecipeCard({ recipe }: { recipe: Recipe }) {
  const authorHandle = recipe.author?.contact || null;
  const [showParamPrompt, setShowParamPrompt] = useState(false);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  const requiredParams = recipe.parameters?.filter((p) => p.requirement === "required") || [];
  const optionalParams = recipe.parameters?.filter((p) => p.requirement !== "required") || [];
  const hasRequiredParams = requiredParams.length > 0;

  const handleCopyCLI = () => {
    if (hasRequiredParams) {
      setParamValues({});
      setShowParamPrompt(true);
      return;
    }
    const command = buildRecipeCliCommand(recipe.localPath);
    navigator.clipboard.writeText(command);
    toast.success("CLI command copied!");
  };

  const handleSubmitParams = () => {
    const filledParams = Object.entries(paramValues)
      .map(([key, val]) => `${key}=${val}`)
      .join(" ");
    const command = buildRecipeCliCommand(recipe.localPath, filledParams);
    navigator.clipboard.writeText(command);
    setShowParamPrompt(false);
    toast.success("CLI command copied with params!");
  };

  return (
    <div className="relative w-full h-full">
      <Link
        to={`/recipes/detail?id=${recipe.id}`}
        className="block no-underline hover:no-underline h-full"
      >
        <div className="absolute inset-0 rounded-2xl bg-purple-500 opacity-10 blur-2xl" />

        <div className="relative z-10 w-full h-full rounded-2xl border border-zinc-200 dark:border-zinc-700 bg-white dark:bg-[#1A1A1A] flex flex-col justify-between p-6 transition-shadow duration-200 ease-in-out hover:shadow-[0_0_0_2px_rgba(99,102,241,0.4),_0_4px_20px_rgba(99,102,241,0.1)]">
          <div className="space-y-4">
            <div>
              <h3 className="font-semibold text-base text-zinc-900 dark:text-white leading-snug">
                {recipe.title}
              </h3>
              <p className="text-sm text-zinc-600 dark:text-zinc-400 mt-1">
                {recipe.description}
              </p>
            </div>

            {recipe.extensions.map((extObj, index) => {
              const name = typeof extObj === 'string' ? extObj : extObj.name;
              const cleanedLabel = name?.replace(/MCP/i, "").trim();

              return (
                <span
                  key={index}
                  className="inline-flex items-center h-7 px-3 rounded-full border border-zinc-300 bg-zinc-100 text-zinc-700 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300 text-xs font-medium"
                >
                  {cleanedLabel}
                </span>
              );
            })}

            {recipe.activities?.filter(a => !a.startsWith('message:')).length > 0 && (
              <div className="border-t border-zinc-200 dark:border-zinc-700 pt-2 mt-2 flex flex-wrap gap-2">
                {recipe.activities.filter(a => !a.startsWith('message:')).map((activity, index) => (
                  <span
                    key={index}
                    className="inline-flex items-center h-7 px-3 rounded-full border border-zinc-300 bg-zinc-100 text-zinc-700 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300 text-xs font-medium"
                  >
                    {activity}
                  </span>
                ))}
              </div>
            )}
          </div>

          <div className="flex justify-between items-center pt-6 mt-2">
            <a
              href={recipe.recipeUrl}
              className="text-sm font-medium text-purple-600 hover:underline dark:text-purple-400"
              target="_blank"
              rel="noopener noreferrer"
              onClick={(e) => e.stopPropagation()}
            >
              Launch in goose Desktop →
            </a>

            <div className="relative group">
              <button
                onClick={(e) => {
                  e.preventDefault();
                  e.stopPropagation();
                  handleCopyCLI();
                }}
                className="text-sm font-medium text-zinc-700 bg-zinc-200 dark:bg-zinc-700 dark:text-white dark:hover:bg-zinc-600 px-3 py-1 rounded hover:bg-zinc-300 cursor-pointer"
              >
                Copy CLI Command
              </button>

              <div className="absolute bottom-full mb-2 left-1/2 -translate-x-1/2 hidden group-hover:block bg-zinc-800 text-white text-xs px-2 py-1 rounded shadow-lg whitespace-nowrap z-50">
                Copies the CLI command to run this recipe
              </div>
            </div>


            {authorHandle && (
              <a
                href={`https://github.com/${authorHandle}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-2 text-sm text-zinc-500 hover:underline dark:text-zinc-300"
                title="Recipe author"
                onClick={(e) => e.stopPropagation()}
              >
                <img
                  src={`https://github.com/${authorHandle}.png`}
                  alt={authorHandle}
                  className="w-5 h-5 rounded-full"
                />
                @{authorHandle}
              </a>
            )}
          </div>
        </div>
      </Link>

      {showParamPrompt && (
        <div className="absolute top-0 left-0 w-full h-full bg-black bg-opacity-70 flex justify-center items-center z-50">
          <div className="bg-white dark:bg-zinc-800 p-6 rounded-lg w-full max-w-md">
            <h3 className="text-lg font-semibold mb-4 text-zinc-900 dark:text-white">Fill in parameters</h3>

            {[...requiredParams, ...optionalParams].map((param) => (
              <div key={param.key} className="mb-3">
                <label className="block text-sm text-zinc-700 dark:text-zinc-200 mb-1">
                  {param.key} {param.requirement !== "required" && <span className="text-zinc-400">(optional)</span>}
                </label>
                <input
                  type="text"
                  value={paramValues[param.key] || ""}
                  onChange={(e) =>
                    setParamValues((prev) => ({ ...prev, [param.key]: e.target.value }))
                  }
                  className="w-full px-3 py-2 border border-zinc-300 dark:border-zinc-600 rounded bg-white dark:bg-zinc-700 text-zinc-900 dark:text-white"
                />
              </div>
            ))}

            <div className="flex justify-end gap-3">
              <button
                onClick={() => setShowParamPrompt(false)}
                className="text-sm text-zinc-500 hover:underline dark:text-zinc-300"
              >
                Cancel
              </button>
              <button
                onClick={handleSubmitParams}
                className="bg-purple-600 text-white px-4 py-2 rounded text-sm hover:bg-purple-700"
              >
                Copy goose CLI Command
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
