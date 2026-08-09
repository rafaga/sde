# TODO

Qué falta por implementar en `sde`, a partir de los 78 archivos `.jsonl`
reales que trae el SDE de EVE Online (variante JSONL, no YAML). Este
documento se genera comparando esa lista real contra lo que
`builder::parser` realmente escribe y lo que `SdeManager` realmente
lee -- no es una lista de deseos, es un inventario.

Ver [ERD.md](ERD.md) para el diagrama de las tablas ya implementadas, y
la [nota de paridad lectura/escritura](#lectura-de-lo-ya-escrito) más
abajo para el detalle de qué tan completa está cada una.

## Ya implementado (17 de 78 archivos)

| Archivo | Tabla(s) | Escritura | Lectura |
|---|---|---|---|
| `categories.jsonl` | `invCategories` | completa | ❌ ninguna |
| `groups.jsonl` | `invGroups` | completa | ❌ ninguna |
| `types.jsonl` | `invTypes`, `typeStar` | completa | ❌ ninguna |
| `races.jsonl` | `races` | completa | ❌ ninguna |
| `npcCorporations.jsonl` | `npcCorporations` + 4 tablas puente | completa | ❌ ninguna |
| `npcCorporationDivisions.jsonl` | `npcCorporationDivisions` | completa | ❌ ninguna |
| `factions.jsonl` | `factions`, `factionRace` | completa | ❌ ninguna |
| `mapRegions.jsonl` | `mapRegions` | completa | 🟡 parcial (solo id/nombre + agregados) |
| `mapConstellations.jsonl` | `mapConstellations` | completa | 🟡 parcial (solo id/nombre/región) |
| `mapSolarSystems.jsonl` | `mapSolarSystems`, `factionSolarSystem` | 🟡 parcial (4 columnas dependen de flags de `dotlan`) | 🟡 parcial |
| `mapStargates.jsonl` | `mapSystemGates`, `mapSystemConnections` (derivada) | completa | 🟡 parcial (solo la tabla derivada) |
| `mapStars.jsonl` | `mapStars` | completa | ❌ ninguna |
| `mapPlanets.jsonl` | `mapPlanets` | completa | 🟡 parcial (falta posición, radius, typeId) |
| `mapMoons.jsonl` | `mapMoons` | completa | 🟡 parcial (falta posición, radius, typeId) |
| `npcStations.jsonl` | `npcStations` | completa | ❌ ninguna |
| `stationOperations.jsonl` | `stationOperations` + 2 tablas puente | completa | ❌ ninguna |
| `stationServices.jsonl` | `stationServices` | completa | ❌ ninguna |

**Dato central de todo este documento**: de estas 17, solo
`mapRegions`/`mapConstellations`/`mapSolarSystems`/`mapStargates`(vía
la tabla derivada)/`mapPlanets`/`mapMoons` tienen *alguna* cobertura de
lectura, y ninguna la tiene completa salvo la tabla dinámica
`mapAbstractSystems` (de `builder::dotlan`, no de un archivo del SDE).
Todo lo demás -- taxonomía de items, razas, facciones, corporaciones,
estrellas, estaciones -- se escribe pero no se puede consultar hoy
desde `SdeManager`.

## Limitaciones conocidas, documentadas en el código

- Solo se lee la variante JSONL del SDE, no la YAML.
- Solo se calcula la proyección 2D isométrica, no la dimétrica, cuando
  falta `position2D` en el dato de origen.
- `npcCorporations.lpOfferTables` no se modela -- referencia un
  dataset (tablas de oferta de puntos de lealtad) que este proyecto no
  tiene.
- `npcCorporations.exchangeRates` no se modela -- presente en solo 1
  de 283 registros reales verificados, insuficiente para confirmar su
  forma real.
- Dos corporaciones reales (Doomheim, InterBus) tienen un `stationId`
  que no resuelve a ninguna estación real -- se limpia a `NULL`
  automáticamente al construir la base, no es un error.

## Sin implementar (61 de 78 archivos)

Agrupados por tema, no por orden alfabético del SDE, para que sea más
fácil decidir por dónde seguir.

### Mecánica de items (dogma) -- 9 archivos

Atributos y efectos que definen cómo se comporta cada item (daño,
resistencias, bonos de skills, etc.). Es la base de cualquier
calculadora de fitting.

- `dogmaAttributeCategories.jsonl`
- `dogmaAttributes.jsonl`
- `dogmaEffects.jsonl`
- `dogmaUnits.jsonl`
- `typeDogma.jsonl` -- qué atributos/efectos tiene cada type
- `typeBonus.jsonl`
- `dynamicItemAttributes.jsonl` -- rangos de abyssal modules
- `typeMaterials.jsonl` -- reprocesado
- `dbuffCollections.jsonl`

### Taxonomía de items y visual -- 13 archivos

Extiende `invCategories`/`invGroups`/`invTypes`, ya implementadas.

- `marketGroups.jsonl`
- `metaGroups.jsonl` (Tech I/II/Faction/etc.)
- `blueprints.jsonl`
- `compressibleTypes.jsonl`
- `contrabandTypes.jsonl`
- `shipTreeElements.jsonl`
- `shipTreeFactions.jsonl`
- `shipTreeGroups.jsonl`
- `typeElements.jsonl`
- `typeLists.jsonl`
- `graphics.jsonl`
- `graphicMaterialSets.jsonl`
- `icons.jsonl`

### Skins -- 12 archivos

Todo el subsistema SKIN (pintura de naves), sin tocar todavía.

- `skins.jsonl`
- `skinLicenses.jsonl`
- `skinMaterials.jsonl`
- `skinrComponentCategories.jsonl`
- `skinrComponentPointValues.jsonl`
- `skinrComponentRarities.jsonl`
- `skinrComponents.jsonl`
- `skinrSlotCategories.jsonl`
- `skinrSlotConfigurations.jsonl`
- `skinrSlotNames.jsonl`
- `skinrSlots.jsonl`
- `skinrTierThresholds.jsonl`

### Personajes y NPCs -- 10 archivos

- `ancestries.jsonl`
- `archetypes.jsonl`
- `bloodlines.jsonl`
- `characterAttributes.jsonl`
- `characterTitles.jsonl`
- `cloneGrades.jsonl`
- `masteries.jsonl`
- `agentTypes.jsonl`
- `agentsInSpace.jsonl`
- `npcCharacters.jsonl`

### Misiones y contenido narrativo -- 9 archivos

- `missions.jsonl`
- `dungeons.jsonl`
- `epicArcs.jsonl`
- `freelanceJobSchemas.jsonl`
- `mercenaryTacticalOperations.jsonl`
- `militaryCampaigns.jsonl`
- `militaryCampaignObjectives.jsonl`
- `sovereigntyUpgrades.jsonl`
- `landmarks.jsonl`

### Extras del mapa -- 5 archivos

Complementan `mapRegions`/`mapConstellations`/`mapSolarSystems`/
`mapPlanets`/`mapMoons`, ya implementadas.

- `mapAsteroidBelts.jsonl`
- `mapSecondarySuns.jsonl`
- `planetResources.jsonl` -- planetary interaction
- `planetSchematics.jsonl` -- planetary interaction
- `controlTowerResources.jsonl` -- POS/estructuras

### Misceláneo -- 3 archivos

- `certificates.jsonl`
- `corporationActivities.jsonl` -- probablemente el lookup de
  `npcCorporations.mainActivityId`/`secondaryActivityId` (ver
  [ERD.md](ERD.md), ahí quedaron como enteros sin FK), a juzgar por el
  nombre -- no confirmado contra un archivo real, no se subió una
  muestra durante esta sesión.
- `translationLanguages.jsonl` -- lista de los 8 idiomas que ya se
  usan en todo el resto del parser (`en`, `es`, `de`, `fr`, `ja`,
  `ko`, `ru`, `zh`); no es una fuente de datos de juego en sí misma.

## Lectura de lo ya escrito

Antes de sumar más archivos de escritura, vale la pena considerar que
la brecha más grande hoy no es de cobertura de archivos sino de
lectura: de las 29 tablas que ya existen en el schema (27 estáticas +
2 dinámicas de `builder::dotlan`), solo `mapAbstractSystems` tiene
cobertura de lectura completa. Sumar más archivos sin ampliar
`SdeManager` en paralelo hace crecer esa brecha, no la achica.
