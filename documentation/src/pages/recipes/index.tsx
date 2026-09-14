import { RecipeCard, Recipe } from "@site/src/components/recipe-card";
import { searchRecipes } from "@site/src/utils/recipes";
import { useState, useEffect } from "react";
import { motion } from "framer-motion";
import Layout from "@theme/Layout";
import Admonition from "@theme/Admonition";
import { Button } from "@site/src/components/ui/button";
import { SidebarFilter, type SidebarFilterGroup } from "@site/src/components/ui/sidebar-filter";
import { Menu, X } from "lucide-react";
import Link from '@docusaurus/Link';

export default function RecipePage() {
  const [recipes, setRecipes] = useState<Recipe[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [selectedFilters, setSelectedFilters] = useState<Record<string, string[]>>({});
  const [isMobileFilterOpen, setIsMobileFilterOpen] = useState(false);
  const [isLoading, setIsLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [currentPage, setCurrentPage] = useState(1);
  const recipesPerPage = 10;

  const uniqueExtensions = Array.from(
    new Set(
      recipes.flatMap((r) =>
        r.extensions?.length
          ? r.extensions.map((ext) =>
              (typeof ext === "string" ? ext : ext.name).toLowerCase().replace(/\s+/g, "-")
            )
          : []
      )
    )
  ).map((ext) => {
    const cleanValue = ext.replace(/-mcp$/, "");
    let label = cleanValue.replace(/-/g, " ");
    if (label.toLowerCase() === "github") {
      label = "GitHub";
    } else {
      label = label.replace(/\b\w/g, (l) => l.toUpperCase());
    }
    return {
      label,
      value: ext
    };
  });

  const sidebarFilterGroups: SidebarFilterGroup[] = [
    {
      title: "Extensions Used",
      options: uniqueExtensions
    }
  ];

  useEffect(() => {
    const loadRecipes = async () => {
      try {
        setIsLoading(true);
        setError(null);
        const results = await searchRecipes(searchQuery);
        setRecipes(results);
      } catch (err) {
        const errorMessage = err instanceof Error ? err.message : "Unknown error";
        setError(`Failed to load recipes: ${errorMessage}`);
        console.error("Error loading recipes:", err);
      } finally {
        setIsLoading(false);
      }
    };

    const timeoutId = setTimeout(loadRecipes, 300);
    return () => clearTimeout(timeoutId);
  }, [searchQuery]);

  let filteredRecipes = recipes;

  Object.entries(selectedFilters).forEach(([group, values]) => {
    if (values.length > 0) {
      filteredRecipes = filteredRecipes.filter((r) => {
        if (group === "Extensions Used") {
          return r.extensions?.some((ext) => {
            const extName = typeof ext === "string" ? ext : ext.name;
            return values.includes(extName.toLowerCase().replace(/\s+/g, "-"));
          }) ?? false;
        }
        return true;
      });
    }
  });

  return (
    <Layout>
      <div className="container mx-auto px-4 py-8 md:p-24">
        <div className="pb-8 md:pb-16">
          <div className="mb-4">
            <h1 className="text-4xl md:text-[64px] font-medium text-textProminent">
              Recipes Cookbook
            </h1>
          </div>
          <p className="text-textProminent">
            Save time and skip setup. Launch any{" "}
            <Link to="/docs/guides/recipes/session-recipes" className="text-purple-600 hover:underline">
              goose recipe
            </Link>{" "}
            in this archived community collection with a single click. The public submission program has ended, and we are not accepting new community recipe submissions.
          </p>
        </div>

        <div className="search-container mb-6 md:mb-8">
          <input
            className="bg-bgApp font-light text-textProminent placeholder-textPlaceholder w-full px-3 py-2 md:py-3 text-2xl md:text-[40px] leading-tight md:leading-[52px] border-b border-borderSubtle focus:outline-none focus:ring-purple-500 focus:border-borderProminent caret-[#FF4F00] pl-0"
            placeholder="Search for recipes by keyword"
            value={searchQuery}
            onChange={(e) => {
              setSearchQuery(e.target.value);
              setCurrentPage(1);
            }}
          />
        </div>

        <div className="md:hidden mb-4">
          <Button onClick={() => setIsMobileFilterOpen(!isMobileFilterOpen)}>
            {isMobileFilterOpen ? <X size={20} /> : <Menu size={20} />}
            {isMobileFilterOpen ? "Close Filters" : "Show Filters"}
          </Button>
        </div>

        <div className="flex flex-col md:flex-row gap-8">
          <div className={`${isMobileFilterOpen ? "block" : "hidden"} md:block md:w-64 mt-6`}>
            <SidebarFilter
              groups={sidebarFilterGroups}
              selectedValues={selectedFilters}
              onChange={(group, values) => {
                setSelectedFilters(prev => ({ ...prev, [group]: values }));
                setCurrentPage(1);
              }}
            />
          </div>

          <div className="flex-1">
            <div className={`${searchQuery ? "pb-2" : "pb-4 md:pb-8"}`}>
              <p className="text-gray-600">
                {searchQuery
                  ? `${filteredRecipes.length} result${filteredRecipes.length !== 1 ? "s" : ""} for "${searchQuery}"`
                  : ""}
              </p>
            </div>

            {error && (
              <Admonition type="danger" title="Error">
                <p>{error}</p>
              </Admonition>
            )}

            {isLoading ? (
              <div className="py-8 text-xl text-gray-600">Loading recipes...</div>
            ) : filteredRecipes.length === 0 ? (
              <Admonition type="info">
                <p>
                  {searchQuery
                    ? "No recipes found matching your search."
                    : "No recipes have been submitted yet."}
                </p>
              </Admonition>
            ) : (
              <>
                <div className="grid grid-cols-1 lg:grid-cols-2 gap-4 md:gap-6">
                  {filteredRecipes
                    .slice((currentPage - 1) * recipesPerPage, currentPage * recipesPerPage)
                    .map((recipe) => (
                      <motion.div
                        key={recipe.id}
                        initial={{ opacity: 0 }}
                        animate={{ opacity: 1 }}
                        exit={{ opacity: 0 }}
                        transition={{ duration: 0.6 }}
                      >
                        <RecipeCard recipe={recipe} />
                      </motion.div>
                    ))}
                </div>

                {filteredRecipes.length > recipesPerPage && (
                  <div className="flex justify-center items-center gap-2 md:gap-4 mt-6 md:mt-8">
                    <Button
                      onClick={() => setCurrentPage(prev => Math.max(prev - 1, 1))}
                      disabled={currentPage === 1}
                      className="px-3 md:px-4 py-2 rounded-md border border-border bg-surfaceHighlight hover:bg-surface text-textProminent disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm md:text-base"
                    >
                      Previous
                    </Button>

                    <span className="text-textProminent text-sm md:text-base">
                      Page {currentPage} of {Math.ceil(filteredRecipes.length / recipesPerPage)}
                    </span>

                    <Button
                      onClick={() => setCurrentPage(prev => Math.min(Math.ceil(filteredRecipes.length / recipesPerPage), prev + 1))}
                      disabled={currentPage >= Math.ceil(filteredRecipes.length / recipesPerPage)}
                      className="px-3 md:px-4 py-2 rounded-md border border-border bg-surfaceHighlight hover:bg-surface text-textProminent disabled:opacity-50 disabled:cursor-not-allowed transition-colors text-sm md:text-base"
                    >
                      Next
                    </Button>
                  </div>
                )}
              </>
            )}
          </div>
        </div>
      </div>
    </Layout>
  );
}
