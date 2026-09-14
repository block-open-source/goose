import { defineMessages, useIntl } from '../../i18n';
import type { Recipe, RecipeExtension } from '../../recipe';
import { containsInvisibleCharacters } from '../../utils/invisibleCharacters';
import { VisibleText } from './VisibleText';

const i18n = defineMessages({
  heading: {
    id: 'recipeExecutionDetails.heading',
    defaultMessage: 'What this recipe will run',
  },
  description: {
    id: 'recipeExecutionDetails.description',
    defaultMessage:
      'Everything below runs with your permissions as soon as you continue. Review each command, endpoint, and file.',
  },
  extensionsHeading: {
    id: 'recipeExecutionDetails.extensionsHeading',
    defaultMessage: 'Extensions',
  },
  noExtensions: {
    id: 'recipeExecutionDetails.noExtensions',
    defaultMessage:
      'This recipe declares no extensions, so your configured default extensions are used.',
  },
  command: {
    id: 'recipeExecutionDetails.command',
    defaultMessage: 'Command',
  },
  argument: {
    id: 'recipeExecutionDetails.argument',
    defaultMessage: 'Argument {index}',
  },
  workingDir: {
    id: 'recipeExecutionDetails.workingDir',
    defaultMessage: 'Working directory',
  },
  environment: {
    id: 'recipeExecutionDetails.environment',
    defaultMessage: 'Environment',
  },
  secretsFromKeychain: {
    id: 'recipeExecutionDetails.secretsFromKeychain',
    defaultMessage: 'Secrets read from your keychain',
  },
  endpoint: {
    id: 'recipeExecutionDetails.endpoint',
    defaultMessage: 'Endpoint',
  },
  headers: {
    id: 'recipeExecutionDetails.headers',
    defaultMessage: 'Headers',
  },
  socket: {
    id: 'recipeExecutionDetails.socket',
    defaultMessage: 'Socket',
  },
  retryHeading: {
    id: 'recipeExecutionDetails.retryHeading',
    defaultMessage: 'Shell commands run between attempts',
  },
  successCheck: {
    id: 'recipeExecutionDetails.successCheck',
    defaultMessage: 'Success check',
  },
  onFailure: {
    id: 'recipeExecutionDetails.onFailure',
    defaultMessage: 'On failure',
  },
  subRecipesHeading: {
    id: 'recipeExecutionDetails.subRecipesHeading',
    defaultMessage: 'Sub-recipes loaded from disk',
  },
  parametersHeading: {
    id: 'recipeExecutionDetails.parametersHeading',
    defaultMessage: 'Parameters',
  },
  parametersNote: {
    id: 'recipeExecutionDetails.parametersNote',
    defaultMessage:
      'Parameter values are substituted into the placeholders above before anything runs. You will be asked for any that are missing.',
  },
  parameterDefault: {
    id: 'recipeExecutionDetails.parameterDefault',
    defaultMessage: 'default',
  },
  parameterFromLink: {
    id: 'recipeExecutionDetails.parameterFromLink',
    defaultMessage: 'set by this link',
  },
  parameterAsked: {
    id: 'recipeExecutionDetails.parameterAsked',
    defaultMessage: 'asked when the recipe starts',
  },
  invisibleCharactersNote: {
    id: 'recipeExecutionDetails.invisibleCharactersNote',
    defaultMessage:
      'Highlighted markers are invisible or direction-changing characters in this recipe, shown by Unicode code point.',
  },
});

interface RecipeExecutionDetailsProps {
  recipe: Recipe;
  providedParameters?: Record<string, string>;
}

function Field({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex gap-3 font-mono text-xs">
      <span className="w-36 shrink-0 text-text-muted">{label}</span>
      <VisibleText text={value} className="text-text-primary" />
    </div>
  );
}

function KeyValueFields({ label, entries }: { label: string; entries: Record<string, string> }) {
  return (
    <>
      {Object.entries(entries).map(([key, value]) => (
        <Field key={key} label={label} value={`${key}=${value}`} />
      ))}
    </>
  );
}

function ExtensionDetails({ extension }: { extension: RecipeExtension }) {
  const intl = useIntl();

  return (
    <li className="rounded border border-border-default p-3 space-y-1">
      <div className="flex items-baseline gap-2 text-sm">
        <VisibleText text={extension.name} className="font-medium text-text-primary" />
        <span className="font-mono text-xs text-text-muted">{extension.type}</span>
      </div>
      {extension.type === 'stdio' && (
        <>
          <Field label={intl.formatMessage(i18n.command)} value={extension.cmd} />
          {(extension.args ?? []).map((arg, index) => (
            <Field
              key={index}
              label={intl.formatMessage(i18n.argument, { index: index + 1 })}
              value={arg}
            />
          ))}
          {extension.cwd && (
            <Field label={intl.formatMessage(i18n.workingDir)} value={extension.cwd} />
          )}
          <KeyValueFields
            label={intl.formatMessage(i18n.environment)}
            entries={extension.envs ?? {}}
          />
          {(extension.env_keys ?? []).map((key) => (
            <Field key={key} label={intl.formatMessage(i18n.secretsFromKeychain)} value={key} />
          ))}
        </>
      )}
      {extension.type === 'streamable_http' && (
        <>
          <Field label={intl.formatMessage(i18n.endpoint)} value={extension.uri} />
          <KeyValueFields
            label={intl.formatMessage(i18n.headers)}
            entries={extension.headers ?? {}}
          />
          {extension.socket && (
            <Field label={intl.formatMessage(i18n.socket)} value={extension.socket} />
          )}
          <KeyValueFields
            label={intl.formatMessage(i18n.environment)}
            entries={extension.envs ?? {}}
          />
          {(extension.env_keys ?? []).map((key) => (
            <Field key={key} label={intl.formatMessage(i18n.secretsFromKeychain)} value={key} />
          ))}
        </>
      )}
    </li>
  );
}

function collectAuthoredText(recipe: Recipe): string[] {
  const text: string[] = [];
  for (const extension of recipe.extensions ?? []) {
    text.push(extension.name);
    if (extension.type === 'stdio') {
      text.push(extension.cmd, ...(extension.args ?? []), extension.cwd ?? '');
      text.push(...Object.entries(extension.envs ?? {}).flat(), ...(extension.env_keys ?? []));
    } else if (extension.type === 'streamable_http') {
      text.push(extension.uri, extension.socket ?? '');
      text.push(...Object.entries(extension.headers ?? {}).flat());
      text.push(...Object.entries(extension.envs ?? {}).flat(), ...(extension.env_keys ?? []));
    }
  }
  for (const check of recipe.retry?.checks ?? []) {
    text.push(check.command);
  }
  text.push(recipe.retry?.on_failure ?? '');
  for (const subRecipe of recipe.sub_recipes ?? []) {
    text.push(subRecipe.name, subRecipe.path);
  }
  for (const parameter of recipe.parameters ?? []) {
    text.push(parameter.key, parameter.default ?? '');
  }
  return text;
}

export function RecipeExecutionDetails({ recipe, providedParameters }: RecipeExecutionDetailsProps) {
  const intl = useIntl();
  const extensions = recipe.extensions ?? [];
  const retryChecks = recipe.retry?.checks ?? [];
  const onFailure = recipe.retry?.on_failure;
  const subRecipes = recipe.sub_recipes ?? [];
  const parameters = recipe.parameters ?? [];
  const hasInvisibleCharacters = collectAuthoredText(recipe).some(containsInvisibleCharacters);

  return (
    <section className="space-y-4" data-testid="recipe-execution-details">
      <div>
        <h3 className="font-medium text-text-primary">{intl.formatMessage(i18n.heading)}</h3>
        <p className="text-sm text-text-muted">{intl.formatMessage(i18n.description)}</p>
      </div>

      {hasInvisibleCharacters && (
        <p className="rounded border border-yellow-200 bg-yellow-50 p-3 text-sm text-yellow-700 dark:border-yellow-800 dark:bg-yellow-900/20 dark:text-yellow-300">
          {intl.formatMessage(i18n.invisibleCharactersNote)}
        </p>
      )}

      <div>
        <h4 className="mb-2 text-sm font-medium text-text-primary">
          {intl.formatMessage(i18n.extensionsHeading)}
        </h4>
        {extensions.length === 0 ? (
          <p className="text-sm text-text-muted">{intl.formatMessage(i18n.noExtensions)}</p>
        ) : (
          <ul className="space-y-2">
            {extensions.map((extension, index) => (
              <ExtensionDetails key={`${extension.name}-${index}`} extension={extension} />
            ))}
          </ul>
        )}
      </div>

      {(retryChecks.length > 0 || onFailure) && (
        <div>
          <h4 className="mb-2 text-sm font-medium text-text-primary">
            {intl.formatMessage(i18n.retryHeading)}
          </h4>
          <div className="rounded border border-border-default p-3 space-y-1">
            {retryChecks.map((check, index) => (
              <Field
                key={index}
                label={intl.formatMessage(i18n.successCheck)}
                value={check.command}
              />
            ))}
            {onFailure && <Field label={intl.formatMessage(i18n.onFailure)} value={onFailure} />}
          </div>
        </div>
      )}

      {subRecipes.length > 0 && (
        <div>
          <h4 className="mb-2 text-sm font-medium text-text-primary">
            {intl.formatMessage(i18n.subRecipesHeading)}
          </h4>
          <div className="rounded border border-border-default p-3 space-y-1">
            {subRecipes.map((subRecipe) => (
              <Field key={subRecipe.name} label={subRecipe.name} value={subRecipe.path} />
            ))}
          </div>
        </div>
      )}

      {parameters.length > 0 && (
        <div>
          <h4 className="mb-1 text-sm font-medium text-text-primary">
            {intl.formatMessage(i18n.parametersHeading)}
          </h4>
          <p className="mb-2 text-sm text-text-muted">{intl.formatMessage(i18n.parametersNote)}</p>
          <div className="rounded border border-border-default p-3 space-y-1">
            {parameters.map((parameter) => {
              const provided = providedParameters?.[parameter.key];
              const value =
                provided !== undefined
                  ? `${provided} (${intl.formatMessage(i18n.parameterFromLink)})`
                  : parameter.default != null
                    ? `${parameter.default} (${intl.formatMessage(i18n.parameterDefault)})`
                    : `(${intl.formatMessage(i18n.parameterAsked)})`;
              return <Field key={parameter.key} label={parameter.key} value={value} />;
            })}
          </div>
        </div>
      )}
    </section>
  );
}
