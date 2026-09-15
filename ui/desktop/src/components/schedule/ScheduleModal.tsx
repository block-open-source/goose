import React, { useState, useEffect, useMemo, FormEvent, useCallback, useRef } from 'react';
import type { ScheduledJobDto } from '@aaif/goose-acp-client';
import { Card } from '../ui/card';
import { Button } from '../ui/button';
import { Input } from '../ui/input';
import { Select } from '../ui/Select';
import { CronPicker } from './CronPicker';
import { Recipe, parseDeeplink, parseRecipeFromFile } from '../../recipe';
import type { RecipeManifest } from '../../recipe';
import { listSavedRecipes } from '../../recipe/recipe_management';
import ClockIcon from '../../assets/clock-icon.svg';
import { defineMessages, useIntl } from '../../i18n';

const i18n = defineMessages({
  editSchedule: { id: 'scheduleModal.editSchedule', defaultMessage: 'Edit Schedule' },
  createNewSchedule: {
    id: 'scheduleModal.createNewSchedule',
    defaultMessage: 'Create New Schedule',
  },
  nameLabel: { id: 'scheduleModal.nameLabel', defaultMessage: 'Name:' },
  namePlaceholder: {
    id: 'scheduleModal.namePlaceholder',
    defaultMessage: 'e.g., daily-summary-job',
  },
  sourceLabel: { id: 'scheduleModal.sourceLabel', defaultMessage: 'Source:' },
  yaml: { id: 'scheduleModal.yaml', defaultMessage: 'YAML' },
  deepLink: { id: 'scheduleModal.deepLink', defaultMessage: 'Deep link' },
  savedRecipes: { id: 'scheduleModal.savedRecipes', defaultMessage: 'Saved recipes' },
  browseYaml: { id: 'scheduleModal.browseYaml', defaultMessage: 'Browse for YAML file...' },
  selected: { id: 'scheduleModal.selected', defaultMessage: 'Selected: {path}' },
  deepLinkPlaceholder: {
    id: 'scheduleModal.deepLinkPlaceholder',
    defaultMessage: 'Paste goose://recipe link here...',
  },
  selectRecipePlaceholder: {
    id: 'scheduleModal.selectRecipePlaceholder',
    defaultMessage: 'Select a saved recipe',
  },
  alreadyScheduledOption: {
    id: 'scheduleModal.alreadyScheduledOption',
    defaultMessage: '{title} (already scheduled)',
  },
  recipeAlreadyScheduled: {
    id: 'scheduleModal.recipeAlreadyScheduled',
    defaultMessage: 'This recipe already has a schedule. Edit it from the Recipes list.',
  },
  loadingRecipes: { id: 'scheduleModal.loadingRecipes', defaultMessage: 'Loading recipes...' },
  noSavedRecipes: { id: 'scheduleModal.noSavedRecipes', defaultMessage: 'No saved recipes found.' },
  failedLoadRecipes: {
    id: 'scheduleModal.failedLoadRecipes',
    defaultMessage: 'Failed to load recipes.',
  },
  recipeParsed: { id: 'scheduleModal.recipeParsed', defaultMessage: 'Recipe parsed successfully' },
  recipeTitle: { id: 'scheduleModal.recipeTitle', defaultMessage: 'Title: {title}' },
  recipeDescription: {
    id: 'scheduleModal.recipeDescription',
    defaultMessage: 'Description: {description}',
  },
  scheduleLabel: { id: 'scheduleModal.scheduleLabel', defaultMessage: 'Schedule:' },
  cancel: { id: 'scheduleModal.cancel', defaultMessage: 'Cancel' },
  updating: { id: 'scheduleModal.updating', defaultMessage: 'Updating...' },
  creating: { id: 'scheduleModal.creating', defaultMessage: 'Creating...' },
  updateSchedule: { id: 'scheduleModal.updateSchedule', defaultMessage: 'Update Schedule' },
  createSchedule: { id: 'scheduleModal.createSchedule', defaultMessage: 'Create Schedule' },
  invalidDeepLink: {
    id: 'scheduleModal.invalidDeepLink',
    defaultMessage: 'Invalid deep link. Please use a goose://recipe link.',
  },
  failedReadFile: {
    id: 'scheduleModal.failedReadFile',
    defaultMessage: 'Failed to read the selected file.',
  },
  failedParseRecipe: {
    id: 'scheduleModal.failedParseRecipe',
    defaultMessage: 'Failed to parse recipe from file.',
  },
  invalidFileType: {
    id: 'scheduleModal.invalidFileType',
    defaultMessage: 'Invalid file type: Please select a YAML file (.yaml or .yml)',
  },
  scheduleIdRequired: {
    id: 'scheduleModal.scheduleIdRequired',
    defaultMessage: 'Schedule ID is required.',
  },
  provideValidRecipe: {
    id: 'scheduleModal.provideValidRecipe',
    defaultMessage: 'Please provide a valid recipe source.',
  },
});

export type SourceType = 'file' | 'deeplink' | 'saved';

export type NewSchedulePayload =
  | {
      sourceType: 'file' | 'deeplink';
      id: string;
      recipe: Recipe;
      cron: string;
    }
  | {
      sourceType: 'saved';
      recipeId: string;
      cron: string;
    };

interface ScheduleModalProps {
  isOpen: boolean;
  onClose: () => void;
  onSubmit: (payload: NewSchedulePayload | string) => Promise<void>;
  schedule: ScheduledJobDto | null;
  isLoadingExternally: boolean;
  apiErrorExternally: string | null;
  initialDeepLink: string | null;
}

const modalLabelClassName = 'block text-sm font-medium text-text-primary mb-1';

export const ScheduleModal: React.FC<ScheduleModalProps> = ({
  isOpen,
  onClose,
  onSubmit,
  schedule,
  isLoadingExternally,
  apiErrorExternally,
  initialDeepLink,
}) => {
  const intl = useIntl();
  const isEditMode = !!schedule;

  const [scheduleId, setScheduleId] = useState<string>('');
  const [sourceType, setSourceType] = useState<SourceType>('file');
  const [recipeSourcePath, setRecipeSourcePath] = useState<string>('');
  const [deepLinkInput, setDeepLinkInput] = useState<string>('');
  const [parsedRecipe, setParsedRecipe] = useState<Recipe | null>(null);
  const [cronExpression, setCronExpression] = useState<string>('0 0 14 * * *');
  const [internalValidationError, setInternalValidationError] = useState<string | null>(null);
  const [isValid, setIsValid] = useState(true);
  const [savedRecipes, setSavedRecipes] = useState<RecipeManifest[]>([]);
  const [savedRecipesLoading, setSavedRecipesLoading] = useState(false);
  const [savedRecipesError, setSavedRecipesError] = useState<string | null>(null);
  const [selectedSavedRecipeId, setSelectedSavedRecipeId] = useState<string | null>(null);
  const sourceRequestIdRef = useRef(0);

  const bumpSourceRequest = () => {
    sourceRequestIdRef.current += 1;
    return sourceRequestIdRef.current;
  };

  const setScheduleIdFromTitle = (title: string) => {
    const cleanId = title
      .toLowerCase()
      .replace(/[^a-z0-9-]/g, '-')
      .replace(/-+/g, '-');
    setScheduleId(cleanId);
  };

  const clearRecipeSourceState = useCallback(() => {
    setParsedRecipe(null);
    setRecipeSourcePath('');
    setDeepLinkInput('');
    setSelectedSavedRecipeId(null);
    setInternalValidationError(null);
  }, []);

  const handleSourceTypeChange = useCallback(
    (next: SourceType) => {
      if (next === sourceType) {
        return;
      }
      bumpSourceRequest();
      clearRecipeSourceState();
      setSourceType(next);
    },
    [clearRecipeSourceState, sourceType]
  );

  const handleSelectSavedRecipe = useCallback(
    (manifest: RecipeManifest | null) => {
      bumpSourceRequest();
      if (manifest?.schedule_cron) {
        setSelectedSavedRecipeId(null);
        setParsedRecipe(null);
        setInternalValidationError(intl.formatMessage(i18n.recipeAlreadyScheduled));
        return;
      }
      setSelectedSavedRecipeId(manifest?.id ?? null);
      setInternalValidationError(null);
      if (manifest) {
        setParsedRecipe(manifest.recipe);
      } else {
        setParsedRecipe(null);
      }
    },
    [intl]
  );

  const handleDeepLinkChange = useCallback(
    async (value: string) => {
      const requestId = bumpSourceRequest();
      setDeepLinkInput(value);
      setInternalValidationError(null);

      if (value.trim()) {
        try {
          const recipe = await parseDeeplink(value.trim());
          if (requestId !== sourceRequestIdRef.current) {
            return;
          }
          if (!recipe) throw new Error();
          setParsedRecipe(recipe);
          if (recipe.title) {
            setScheduleIdFromTitle(recipe.title);
          }
        } catch {
          if (requestId !== sourceRequestIdRef.current) {
            return;
          }
          setParsedRecipe(null);
          setInternalValidationError(intl.formatMessage(i18n.invalidDeepLink));
        }
      } else {
        setParsedRecipe(null);
      }
    },
    [intl]
  );

  useEffect(() => {
    if (isOpen) {
      setInternalValidationError(null);
      if (schedule) {
        setScheduleId(schedule.id);
        setCronExpression(schedule.cron);
      } else {
        bumpSourceRequest();
        setScheduleId('');
        setSourceType('file');
        setRecipeSourcePath('');
        setDeepLinkInput('');
        setParsedRecipe(null);
        setCronExpression('0 0 14 * * *');
        setSavedRecipes([]);
        setSavedRecipesLoading(false);
        setSavedRecipesError(null);
        setSelectedSavedRecipeId(null);
        if (initialDeepLink) {
          setSourceType('deeplink');
          handleDeepLinkChange(initialDeepLink);
        }
      }
    }
  }, [isOpen, schedule, initialDeepLink, handleDeepLinkChange]);

  useEffect(() => {
    if (sourceType !== 'saved') {
      return;
    }
    let cancelled = false;
    setSavedRecipesLoading(true);
    setSavedRecipesError(null);
    listSavedRecipes()
      .then((recipes) => {
        if (!cancelled) {
          setSavedRecipes(recipes);
          setSavedRecipesLoading(false);
        }
      })
      .catch(() => {
        if (!cancelled) {
          setSavedRecipesError(intl.formatMessage(i18n.failedLoadRecipes));
          setSavedRecipesLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [sourceType, intl]);

  const savedRecipeOptions = useMemo(
    () =>
      savedRecipes.map((m) => ({
        value: m.id,
        label: m.schedule_cron
          ? intl.formatMessage(i18n.alreadyScheduledOption, { title: m.recipe.title || m.id })
          : m.recipe.title || m.id,
      })),
    [savedRecipes, intl]
  );

  const handleBrowseFile = async () => {
    const fileResponse = await window.electron.selectRecipeFile();
    if (fileResponse) {
      if (fileResponse.filePath.endsWith('.yaml') || fileResponse.filePath.endsWith('.yml')) {
        const requestId = bumpSourceRequest();
        setRecipeSourcePath(fileResponse.filePath);
        setInternalValidationError(null);

        try {
          if (requestId !== sourceRequestIdRef.current) {
            return;
          }
          if (!fileResponse.found || fileResponse.error) {
            throw new Error(intl.formatMessage(i18n.failedReadFile));
          }
          const recipe = await parseRecipeFromFile(fileResponse.file);
          if (requestId !== sourceRequestIdRef.current) {
            return;
          }
          if (!recipe) {
            throw new Error(intl.formatMessage(i18n.failedParseRecipe));
          }
          setParsedRecipe(recipe);
          if (recipe.title) {
            setScheduleIdFromTitle(recipe.title);
          }
        } catch (e) {
          if (requestId !== sourceRequestIdRef.current) {
            return;
          }
          setParsedRecipe(null);
          setInternalValidationError(
            e instanceof Error ? e.message : intl.formatMessage(i18n.failedParseRecipe)
          );
        }
      } else {
        setInternalValidationError(intl.formatMessage(i18n.invalidFileType));
      }
    }
  };

  const handleLocalSubmit = async (event: FormEvent) => {
    event.preventDefault();
    setInternalValidationError(null);

    if (isEditMode) {
      await onSubmit(cronExpression);
      return;
    }

    if (sourceType === 'saved') {
      const selected = savedRecipes.find((m) => m.id === selectedSavedRecipeId);
      if (!selected) {
        setInternalValidationError(intl.formatMessage(i18n.provideValidRecipe));
        return;
      }
      if (selected.schedule_cron) {
        setInternalValidationError(intl.formatMessage(i18n.recipeAlreadyScheduled));
        return;
      }
      await onSubmit({
        sourceType: 'saved',
        recipeId: selected.id,
        cron: cronExpression,
      });
      return;
    }

    if (!scheduleId.trim()) {
      setInternalValidationError(intl.formatMessage(i18n.scheduleIdRequired));
      return;
    }

    if (!parsedRecipe) {
      setInternalValidationError(intl.formatMessage(i18n.provideValidRecipe));
      return;
    }

    if (sourceType === 'file' && !recipeSourcePath) {
      setInternalValidationError(intl.formatMessage(i18n.provideValidRecipe));
      return;
    }

    if (sourceType === 'deeplink' && !deepLinkInput.trim()) {
      setInternalValidationError(intl.formatMessage(i18n.provideValidRecipe));
      return;
    }

    await onSubmit({
      sourceType,
      id: scheduleId.trim(),
      recipe: parsedRecipe,
      cron: cronExpression,
    });
  };

  if (!isOpen) return null;

  const recipeParsedPreview = parsedRecipe ? (
    <div className="mt-2 p-2 bg-green-100 dark:bg-green-900/30 rounded-md border border-green-500/50">
      <p className="text-xs text-green-700 dark:text-green-300 font-medium">
        ✓ {intl.formatMessage(i18n.recipeParsed)}
      </p>
      <p className="text-xs text-green-600 dark:text-green-400">
        {intl.formatMessage(i18n.recipeTitle, { title: parsedRecipe.title })}
      </p>
      <p className="text-xs text-green-600 dark:text-green-400">
        {intl.formatMessage(i18n.recipeDescription, { description: parsedRecipe.description })}
      </p>
    </div>
  ) : null;

  return (
    <div className="fixed inset-0 bg-black/50 z-40 flex items-center justify-center p-4">
      <Card className="w-full max-w-md bg-background-primary shadow-xl rounded-3xl z-50 flex flex-col max-h-[90vh] overflow-hidden">
        <div className="px-8 pt-6 pb-4 flex-shrink-0">
          <div className="flex items-center gap-3">
            <img src={ClockIcon} alt="Clock" className="w-8 h-8" />
            <div className="flex-1">
              <h2 className="text-base font-semibold text-text-primary">
                {isEditMode
                  ? intl.formatMessage(i18n.editSchedule)
                  : intl.formatMessage(i18n.createNewSchedule)}
              </h2>
              {isEditMode && <p className="text-sm text-text-secondary">{schedule.id}</p>}
            </div>
          </div>
        </div>

        <form
          id="schedule-form"
          onSubmit={handleLocalSubmit}
          className="px-8 py-4 space-y-4 flex-grow overflow-y-auto"
        >
          {apiErrorExternally && (
            <p className="text-text-danger text-sm mb-3 p-2 border border-border-danger rounded-md">
              {apiErrorExternally}
            </p>
          )}
          {internalValidationError && (
            <p className="text-text-danger text-sm mb-3 p-2 border border-border-danger rounded-md">
              {internalValidationError}
            </p>
          )}

          {!isEditMode && (
            <>
              {sourceType !== 'saved' && (
                <div>
                  <label htmlFor="scheduleId-modal" className={modalLabelClassName}>
                    {intl.formatMessage(i18n.nameLabel)} <span className="text-red-500">*</span>
                  </label>
                  <Input
                    type="text"
                    id="scheduleId-modal"
                    value={scheduleId}
                    onChange={(e) => setScheduleId(e.target.value)}
                    placeholder={intl.formatMessage(i18n.namePlaceholder)}
                    required
                  />
                </div>
              )}

              <div>
                <label className={modalLabelClassName}>
                  {intl.formatMessage(i18n.sourceLabel)} <span className="text-red-500">*</span>
                </label>
                <div className="space-y-2">
                  <div className="flex bg-gray-100 dark:bg-gray-700 rounded-full p-1">
                    <button
                      type="button"
                      onClick={() => handleSourceTypeChange('file')}
                      className={`flex-1 px-4 py-2 text-sm font-medium rounded-full transition-all ${
                        sourceType === 'file'
                          ? 'bg-white dark:bg-gray-800 text-gray-900 dark:text-white shadow-sm'
                          : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-white'
                      }`}
                    >
                      {intl.formatMessage(i18n.yaml)}
                    </button>
                    <button
                      type="button"
                      onClick={() => handleSourceTypeChange('deeplink')}
                      className={`flex-1 px-4 py-2 text-sm font-medium rounded-full transition-all ${
                        sourceType === 'deeplink'
                          ? 'bg-white dark:bg-gray-800 text-gray-900 dark:text-white shadow-sm'
                          : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-white'
                      }`}
                    >
                      {intl.formatMessage(i18n.deepLink)}
                    </button>
                    <button
                      type="button"
                      onClick={() => handleSourceTypeChange('saved')}
                      className={`flex-1 px-4 py-2 text-sm font-medium rounded-full transition-all ${
                        sourceType === 'saved'
                          ? 'bg-white dark:bg-gray-800 text-gray-900 dark:text-white shadow-sm'
                          : 'text-gray-600 dark:text-gray-400 hover:text-gray-900 dark:hover:text-white'
                      }`}
                    >
                      {intl.formatMessage(i18n.savedRecipes)}
                    </button>
                  </div>

                  {sourceType === 'file' && (
                    <div>
                      <Button
                        type="button"
                        variant="outline"
                        onClick={handleBrowseFile}
                        className="w-full justify-center rounded-full"
                      >
                        {intl.formatMessage(i18n.browseYaml)}
                      </Button>
                      {recipeSourcePath && (
                        <p className="mt-2 text-xs text-gray-500 dark:text-gray-400 italic">
                          {intl.formatMessage(i18n.selected, { path: recipeSourcePath })}
                        </p>
                      )}
                    </div>
                  )}

                  {sourceType === 'deeplink' && (
                    <div>
                      <Input
                        type="text"
                        value={deepLinkInput}
                        onChange={(e) => handleDeepLinkChange(e.target.value)}
                        placeholder={intl.formatMessage(i18n.deepLinkPlaceholder)}
                        className="rounded-full"
                      />
                      {recipeParsedPreview}
                    </div>
                  )}

                  {sourceType === 'saved' && (
                    <div data-testid="saved-recipe-picker">
                      {savedRecipesError ? (
                        <p className="text-sm text-text-danger">{savedRecipesError}</p>
                      ) : savedRecipes.length === 0 ? (
                        <p className="text-sm text-text-secondary">
                          {savedRecipesLoading
                            ? intl.formatMessage(i18n.loadingRecipes)
                            : intl.formatMessage(i18n.noSavedRecipes)}
                        </p>
                      ) : (
                        <Select
                          options={savedRecipeOptions}
                          value={
                            savedRecipeOptions.find((o) => o.value === selectedSavedRecipeId) ??
                            null
                          }
                          onChange={(option) => {
                            const selectedId = (option as { value?: string } | null)?.value ?? null;
                            handleSelectSavedRecipe(
                              selectedId
                                ? (savedRecipes.find((m) => m.id === selectedId) ?? null)
                                : null
                            );
                          }}
                          placeholder={intl.formatMessage(i18n.selectRecipePlaceholder)}
                        />
                      )}
                      {recipeParsedPreview}
                    </div>
                  )}
                </div>
              </div>
            </>
          )}

          <div>
            <label className={modalLabelClassName}>{intl.formatMessage(i18n.scheduleLabel)}</label>
            <CronPicker schedule={schedule} onChange={setCronExpression} isValid={setIsValid} />
          </div>
        </form>

        <div className="flex gap-2 px-8 py-4 border-t border-border-primary">
          <Button
            type="button"
            variant="ghost"
            onClick={onClose}
            disabled={isLoadingExternally}
            className="flex-1 text-gray-400 hover:bg-gray-50 dark:hover:bg-gray-800"
          >
            {intl.formatMessage(i18n.cancel)}
          </Button>
          <Button
            type="submit"
            form="schedule-form"
            disabled={isLoadingExternally || !isValid}
            className="flex-1"
          >
            {isLoadingExternally
              ? isEditMode
                ? intl.formatMessage(i18n.updating)
                : intl.formatMessage(i18n.creating)
              : isEditMode
                ? intl.formatMessage(i18n.updateSchedule)
                : intl.formatMessage(i18n.createSchedule)}
          </Button>
        </div>
      </Card>
    </div>
  );
};
