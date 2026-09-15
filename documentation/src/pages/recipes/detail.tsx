import Layout from "@theme/Layout";
import { ArrowLeft } from "lucide-react";
import { useLocation } from "@docusaurus/router";
import { useEffect, useState } from "react";
import Link from "@docusaurus/Link";
import Admonition from "@theme/Admonition";
import CodeBlock from "@theme/CodeBlock";
import { Button } from "@site/src/components/ui/button";
import { getRecipeById } from "@site/src/utils/recipes";
import type { Recipe } from "@site/src/components/recipe-card";
import toast from "react-hot-toast";
import ReactMarkdown from "react-markdown";
import { buildRecipeCliCommand } from "@site/src/utils/recipe-command";

const colorMap: { [key: string]: string } = {
  "GitHub MCP": "bg-yellow-100 text-yellow-800 border-yellow-200",
  "Context7 MCP": "bg-purple-100 text-purple-800 border-purple-200",
  "Memory": "bg-blue-100 text-blue-800 border-blue-200",
};

export default function RecipeDetailPage(): JSX.Element {
  const location = useLocation();
  const [recipe, setRecipe] = useState<Recipe | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showParamsPrompt, setShowParamsPrompt] = useState(false);
  const [paramValues, setParamValues] = useState<Record<string, string>>({});

  useEffect(() => {
    const loadRecipe = async () => {
      try {
        setLoading(true);
        setError(null);
        setParamValues({});
        setShowParamsPrompt(false);

        const params = new URLSearchParams(location.search);
        const id = params.get("id");
        if (!id) {
          setError("No recipe ID provided");
          return;
        }

        const recipeData = await getRecipeById(id);
        if (recipeData) {
          setRecipe(recipeData);
        } else {
          setError("Recipe not found");
        }
      } catch (err) {
        setError("Failed to load recipe details");
        console.error(err);
      } finally {
        setLoading(false);
      }
    };

    loadRecipe();
  }, [location]);

  const allParams = recipe?.parameters || [];
  const requiredParams = allParams.filter((p) => p.requirement === "required");

  const handleCopyCLI = () => {
    if (!recipe) return;

    if (allParams.length > 0) {
      setParamValues({});
      setShowParamsPrompt(true);
      return;
    }

    const command = buildRecipeCliCommand(recipe.localPath);
    navigator.clipboard.writeText(command);
    toast.success("CLI command copied!");
  };

  const handleSubmitParams = () => {
    if (!recipe) return;

    const filledParams = Object.entries(paramValues)
      .filter(([, val]) => val !== "")
      .map(([key, val]) => `${key}=${val}`)
      .join(" ");

    const command = buildRecipeCliCommand(recipe.localPath, filledParams);

    navigator.clipboard.writeText(command);
    toast.success("CLI command copied with params!");
    setShowParamsPrompt(false);
  };

  if (loading) {
    return (
      <Layout>
        <div className="min-h-screen flex items-start justify-center py-16">
          <div className="container max-w-5xl mx-auto px-4 animate-pulse">
            <div className="h-12 w-48 bg-bgSubtle dark:bg-zinc-800 rounded-lg mb-4"></div>
            <div className="h-6 w-full bg-bgSubtle dark:bg-zinc-800 rounded-lg mb-2"></div>
            <div className="h-6 w-2/3 bg-bgSubtle dark:bg-zinc-800 rounded-lg"></div>
          </div>
        </div>
      </Layout>
    );
  }

  if (error || !recipe) {
    return (
      <Layout>
        <div className="min-h-screen flex items-start justify-center py-16">
          <div className="container max-w-5xl mx-auto px-4 text-red-500">
            {error || "Recipe not found"}
          </div>
        </div>
      </Layout>
    );
  }

  const authorUsername = typeof recipe.author === "string" ? recipe.author : recipe.author?.contact;

  return (
    <Layout>
      <div className="min-h-screen py-12">
        <div className="max-w-4xl mx-auto px-4">
          <div className="mb-8 flex justify-between items-start">
            <Link to="/recipes">
              <Button className="flex items-center gap-2">
                <ArrowLeft className="h-4 w-4" />
                Back
              </Button>
            </Link>
            {authorUsername && (
              <a
                href={`https://github.com/${authorUsername}`}
                target="_blank"
                rel="noopener noreferrer"
                className="flex items-center gap-2 text-sm text-textSubtle hover:underline"
              >
                <img
                  src={`https://github.com/${authorUsername}.png`}
                  alt={authorUsername}
                  className="w-6 h-6 rounded-full"
                />
                @{authorUsername}
              </a>
            )}
          </div>

          <div className="bg-white dark:bg-[#1A1A1A] border border-borderSubtle dark:border-zinc-700 rounded-xl p-8 shadow-md">
            <h1 className="text-4xl font-semibold mb-2 text-textProminent dark:text-white">
              {recipe.title}
            </h1>
            <p className="text-textSubtle dark:text-zinc-400 text-lg mb-6">{recipe.description}</p>

            {/* Message Activities - rendered as info box */}
            {recipe.activities?.some(a => a.startsWith('message:')) && (
              <div className="mb-6 border-t border-borderSubtle dark:border-zinc-700 pt-6">
                <Admonition type="info">
                  {recipe.activities
                    .filter(a => a.startsWith('message:'))
                    .map((activity, index) => {
                      const messageContent = activity.replace(/^message:\s*/, '');
                      return (
                        <div key={index} className="prose prose-sm dark:prose-invert max-w-none [&_p]:my-2 [&_ul]:my-2 [&_li]:my-1">
                          <ReactMarkdown>{messageContent}</ReactMarkdown>
                        </div>
                      );
                    })}
                </Admonition>
              </div>
            )}

            {/* Regular Activities - rendered as pills */}
            {recipe.activities?.filter(a => !a.startsWith('message:')).length > 0 && (
              <div className="mb-6 border-t border-borderSubtle dark:border-zinc-700 pt-6">
                <h2 className="text-2xl font-medium mb-2 text-textProminent dark:text-white">Activities</h2>
                <div className="flex flex-wrap gap-2">
                  {recipe.activities.filter(a => !a.startsWith('message:')).map((activity, index) => (
                    <span
                      key={index}
                      className="bg-surfaceHighlight dark:bg-zinc-900 border border-border dark:border-zinc-700 rounded-full px-3 py-1 text-sm text-textProminent dark:text-zinc-300"
                    >
                      {activity}
                    </span>
                  ))}
                </div>
              </div>
            )}

            {/* Extensions */}
            {recipe.extensions?.length > 0 && (
              <div className="mb-6 border-t border-borderSubtle dark:border-zinc-700 pt-6">
                <h2 className="text-2xl font-medium mb-2 text-textProminent dark:text-white">Extensions</h2>
                <div className="flex flex-wrap gap-2">
                  {recipe.extensions.map((ext, index) => {
                    const name = typeof ext === "string" ? ext : ext.name;
                    return (
                      <span
                        key={index}
                        className={`border rounded-full px-3 py-1 text-sm ${
                          colorMap[name] ||
                          "bg-gray-100 text-gray-800 border-gray-200 dark:bg-zinc-900 dark:text-zinc-300 dark:border-zinc-700"
                        }`}
                      >
                        {name}
                      </span>
                    );
                  })}
                </div>
              </div>
            )}

            {/* Prompt */}
            {recipe.prompt && (
              <div className="mb-6 border-t border-borderSubtle dark:border-zinc-700 pt-6">
                <h2 className="text-2xl font-medium mb-4 text-textProminent dark:text-white">Initial Prompt</h2>
                <Admonition type="info" className="mb-4">
                  This prompt auto-starts the recipe when launched in Goose.
                </Admonition>
                <CodeBlock language="markdown">{recipe.prompt}</CodeBlock>
              </div>
            )}

            {/* Instructions */}
            {recipe.instructions && (
              <div className="mb-6 border-t border-borderSubtle dark:border-zinc-700 pt-6">
                <h2 className="text-2xl font-medium mb-4 text-textProminent dark:text-white">Instructions</h2>
                <CodeBlock language="markdown">{recipe.instructions}</CodeBlock>
              </div>
            )}

            {/* Launch */}
            <div className="pt-8 border-t border-borderSubtle dark:border-zinc-700 mt-6 flex gap-4">
              <Link
                to={recipe.recipeUrl}
                target="_blank"
                className="inline-block text-white bg-black dark:bg-white dark:text-black px-6 py-2 rounded-full text-sm font-medium hover:bg-gray-900 dark:hover:bg-gray-100 transition-colors"
              >
                Launch in Goose Desktop →
              </Link>
              <div className="relative group inline-block">
                <button
                  onClick={handleCopyCLI}
                  className="text-sm font-medium px-6 py-2 rounded-full bg-zinc-200 dark:bg-zinc-800 text-zinc-700 dark:text-white hover:bg-zinc-300 dark:hover:bg-zinc-700 transition-colors cursor-pointer"
                >
                  Copy Goose CLI Command
                </button>

                <div className="absolute bottom-full mb-2 left-1/2 -translate-x-1/2 hidden group-hover:block bg-zinc-800 text-white text-xs px-2 py-1 rounded shadow-lg whitespace-nowrap z-50">
                  Copies the CLI command to run this recipe
                </div>
              </div>

            </div>
          </div>
        </div>
      </div>

      {showParamsPrompt && (
        <div className="fixed inset-0 bg-black bg-opacity-60 z-50 flex items-center justify-center">
          <div className="bg-white dark:bg-zinc-800 p-6 rounded-lg w-full max-w-md">
            <h3 className="text-lg font-semibold mb-4 text-zinc-900 dark:text-white">Fill in parameters</h3>
            {allParams.map((param) => (
              <div key={param.key} className="mb-3">
                <label className="block text-sm text-zinc-700 dark:text-zinc-200 mb-1">
                  {param.key} {param.requirement === "optional" && <span className="text-zinc-400">(optional)</span>}
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
                onClick={() => setShowParamsPrompt(false)}
                className="text-sm text-zinc-600 dark:text-zinc-300 hover:underline"
              >
                Cancel
              </button>
              <button
                onClick={handleSubmitParams}
                className="bg-purple-600 text-white px-4 py-2 rounded text-sm hover:bg-purple-700"
              >
                Copy Command
              </button>
            </div>
          </div>
        </div>
      )}
    </Layout>
  );
}
