const cache = {
  provider: '',
  models: [],
};

export function cachedModelsFor(provider) {
  if (cache.provider !== provider || cache.models.length === 0) return [];
  return [...cache.models];
}

export function storeCachedModels(provider, models) {
  cache.provider = provider;
  cache.models = [...models];
}
