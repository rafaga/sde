-- ============================================================
-- Esquema corregido (SQLite, modo STRICT) — basado en EVE Online SDE
-- Requiere SQLite >= 3.37.0 (agosto 2021)
-- ============================================================
PRAGMA foreign_keys = ON;

-- ------------------------------------------------------------
-- Inventario
-- ------------------------------------------------------------

CREATE TABLE invCategories (
  categoryId    INTEGER NOT NULL PRIMARY KEY,
  categoryName  TEXT NOT NULL,
  published     INTEGER NOT NULL CHECK (published IN (0,1))
) STRICT;

CREATE TABLE invGroups (
  groupId     INTEGER NOT NULL PRIMARY KEY,
  groupName   TEXT NOT NULL,
  categoryId  INTEGER NOT NULL REFERENCES invCategories(categoryId)
                ON UPDATE CASCADE ON DELETE RESTRICT,
  anchorable  INTEGER NOT NULL CHECK (anchorable IN (0,1))
) STRICT;
CREATE INDEX idx_invGroups_categoryId ON invGroups(categoryId);

CREATE TABLE invTypes (
  typeId     INTEGER NOT NULL PRIMARY KEY,
  groupId    INTEGER REFERENCES invGroups(groupId)
               ON UPDATE CASCADE ON DELETE SET NULL,
  iconId     INTEGER,
  typeName   TEXT NOT NULL,
  published  INTEGER NOT NULL CHECK (published IN (0,1)),
  volume     REAL
) STRICT;
CREATE INDEX idx_invTypes_groupId ON invTypes(groupId);

-- ------------------------------------------------------------
-- Razas / NPCs / Facciones
-- ------------------------------------------------------------

CREATE TABLE races (
  raceId    INTEGER NOT NULL PRIMARY KEY,
  raceName  TEXT NOT NULL
) STRICT;

CREATE TABLE npcCorporations (
  corporationId    INTEGER NOT NULL PRIMARY KEY,
  corporationName  TEXT NOT NULL,
  tickerName       TEXT NOT NULL,
  deleted          INTEGER NOT NULL CHECK (deleted IN (0,1)),
  iconId           INTEGER,
  raceId           INTEGER REFERENCES races(raceId)
                     ON UPDATE CASCADE ON DELETE SET NULL
) STRICT;
CREATE INDEX idx_npcCorporations_raceId ON npcCorporations(raceId);

CREATE TABLE factions (
  factionId      INTEGER NOT NULL PRIMARY KEY,
  factionName    TEXT NOT NULL,
  iconId         INTEGER NOT NULL,
  sizeFactor     REAL NOT NULL,
  uniqueName     INTEGER NOT NULL CHECK (uniqueName IN (0,1)),
  corporationId  INTEGER REFERENCES npcCorporations(corporationId)
                   ON UPDATE CASCADE ON DELETE SET NULL
) STRICT;
CREATE INDEX idx_factions_corporationId ON factions(corporationId);

CREATE TABLE factionRace (
  factionId  INTEGER NOT NULL REFERENCES factions(factionId)
               ON UPDATE CASCADE ON DELETE CASCADE,
  raceId     INTEGER NOT NULL REFERENCES races(raceId)
               ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (factionId, raceId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- ------------------------------------------------------------
-- Mapa: regiones / constelaciones / sistemas
-- ------------------------------------------------------------

CREATE TABLE mapRegions (
  regionId    INTEGER NOT NULL PRIMARY KEY,
  regionName  TEXT NOT NULL,
  nebula      INTEGER NOT NULL,
  wormholeClassId INTEGER,
  factionId   INTEGER REFERENCES factions(factionId)
                ON UPDATE CASCADE ON DELETE SET NULL,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL,
  maxProjX REAL NOT NULL DEFAULT(0.0), maxProjY REAL NOT NULL DEFAULT(0.0)
) STRICT;
CREATE INDEX idx_mapRegions_factionId ON mapRegions(factionId);

CREATE TABLE mapConstellations (
  constellationId  INTEGER NOT NULL PRIMARY KEY,
  constellationName TEXT NOT NULL,
  regionId  INTEGER NOT NULL REFERENCES mapRegions(regionId)
              ON UPDATE CASCADE ON DELETE RESTRICT,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL
) STRICT;
CREATE INDEX idx_mapConstellations_regionId ON mapConstellations(regionId);

CREATE TABLE mapSolarSystems (
  solarSystemId   INTEGER NOT NULL PRIMARY KEY,
  solarSystemName TEXT NOT NULL,
  constellationId INTEGER REFERENCES mapConstellations(constellationId)
                    ON UPDATE CASCADE ON DELETE SET NULL,
  corridor      INTEGER CHECK (corridor IN (0,1)),
  fringe        INTEGER CHECK (fringe IN (0,1)),
  hub           INTEGER CHECK (hub IN (0,1)),
  international INTEGER CHECK (international IN (0,1)),
  luminosity REAL,
  radius REAL NOT NULL,
  centerX REAL NOT NULL, centerY REAL NOT NULL, centerZ REAL NOT NULL,
  position2DX REAL, position2DY REAL,
  regional INTEGER CHECK (regional IN (0,1)),
  security REAL NOT NULL CHECK (security BETWEEN -1.0 AND 1.0),
  securityClass TEXT
) STRICT;
CREATE INDEX idx_mapSolarSystems_constellationId ON mapSolarSystems(constellationId);

CREATE TABLE factionSolarSystem (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  factionId     INTEGER NOT NULL REFERENCES factions(factionId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, factionId)
) STRICT, WITHOUT ROWID;
CREATE UNIQUE INDEX factionId ON factionSolarSystem (factionId);

-- ------------------------------------------------------------
-- Portales / conexiones / cuerpos celestes
-- ------------------------------------------------------------

CREATE TABLE mapSystemGates (
  systemGateId  INTEGER NOT NULL,
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE RESTRICT,
  destinationGateId INTEGER NOT NULL REFERENCES mapSystemGates(systemGateId)
                       ON UPDATE CASCADE ON DELETE SET NULL
                       DEFERRABLE INITIALLY DEFERRED,
  destinationSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                         ON UPDATE CASCADE ON DELETE RESTRICT,
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  CONSTRAINT pkey PRIMARY KEY (systemGateId, solarSystemId) ON CONFLICT FAIL
) STRICT;
CREATE UNIQUE INDEX idx_mapSystemGates_systemGateId ON mapSystemGates(systemGateId);
CREATE INDEX idx_mapSystemGates_solarSystemId ON mapSystemGates(solarSystemId);
CREATE INDEX idx_mapSystemGates_typeId ON mapSystemGates(typeId);

CREATE TABLE mapSystemConnections (
  systemA INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
            ON UPDATE CASCADE ON DELETE RESTRICT,
  systemB INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
            ON UPDATE CASCADE ON DELETE RESTRICT,
  PRIMARY KEY (systemA, systemB),
  CHECK (systemA < systemB)
) STRICT;
CREATE INDEX idx_mapSystemConnections_systemA ON mapSystemConnections(systemA);
CREATE INDEX idx_mapSystemConnections_systemB ON mapSystemConnections(systemB);

CREATE TABLE mapPlanets (
  planetId INTEGER NOT NULL PRIMARY KEY,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE SET NULL,
  planetaryIndex INTEGER NOT NULL,
  fragmented INTEGER CHECK (fragmented IN (0,1)),
  radius REAL,
  locked INTEGER CHECK (locked IN (0,1)),
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL
) STRICT;
CREATE UNIQUE INDEX planetSystem ON mapPlanets (solarSystemId, planetaryIndex);
CREATE INDEX idx_mapPlanets_typeId ON mapPlanets(typeId);

CREATE TABLE typeStar (
  starTypeId INTEGER PRIMARY KEY,
  typeId INTEGER NOT NULL REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE CASCADE,
  name  TEXT NOT NULL CHECK (length(name) <= 4),
  color TEXT NOT NULL
) STRICT;
CREATE INDEX idx_typeStar_typeId ON typeStar(typeId);

CREATE TABLE mapStars (
  starId INTEGER NOT NULL PRIMARY KEY,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE RESTRICT,
  locked INTEGER CHECK (locked IN (0,1)),
  radius INTEGER,
  starTypeId INTEGER NOT NULL REFERENCES typeStar(starTypeId)
               ON UPDATE CASCADE ON DELETE CASCADE
) STRICT;
CREATE UNIQUE INDEX starId ON mapStars (solarSystemId, starId);
CREATE INDEX idx_mapStars_starTypeId ON mapStars(starTypeId);

CREATE TABLE mapMoons (
  moonId INTEGER NOT NULL,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE SET NULL,
  moonIndex INTEGER NOT NULL,
  planetId INTEGER REFERENCES mapPlanets(planetId)
             ON UPDATE CASCADE ON DELETE SET NULL,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  radius INTEGER,
  typeId INTEGER REFERENCES invTypes(typeId)
           ON UPDATE CASCADE ON DELETE SET NULL,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, moonId) ON CONFLICT FAIL
) STRICT;
CREATE UNIQUE INDEX moonId ON mapMoons(moonId);
CREATE INDEX idx_mapMoons_planetId ON mapMoons(planetId);

-- ------------------------------------------------------------
-- Estaciones / corporaciones NPC por sistema
-- ------------------------------------------------------------

-- Community-maintained bits stop here; the tables below cover NPC
-- station data, verified against real npcStations.jsonl (5210
-- records), stationOperations.jsonl (68 records), and
-- stationServices.jsonl (27 records), August 2026.
--
-- Replaces the old staStation/staCorporations declarations: neither was
-- ever populated, by this port or by the original Python prototype
-- (_parse_station() never existed in sde_parser.py) -- confirmed a
-- schema/parser mismatch inherited from the reference implementation,
-- not a gap introduced here. The real SDE export uses different table
-- names (npcStations, not staStation) and a materially different, richer
-- shape, so this isn't a rename -- it's a fresh design against the
-- actual data.

CREATE TABLE stationServices (
  serviceId    INTEGER NOT NULL PRIMARY KEY,
  serviceName  TEXT NOT NULL
) STRICT;

CREATE TABLE stationOperations (
  operationId          INTEGER NOT NULL PRIMARY KEY,
  activityId           INTEGER NOT NULL,
  operationName        TEXT NOT NULL,
  -- Nullable: present in 55/68 real records (80.9%).
  description          TEXT,
  -- Likelihood of this operation appearing in each map-zone type
  -- (border/corridor/fringe/hub of the *region*, unrelated to the
  -- identically-named mapSolarSystems columns, which describe a
  -- specific solar system's own zone membership, not an operation's
  -- affinity for a zone type).
  border               REAL NOT NULL,
  corridor             REAL NOT NULL,
  fringe               REAL NOT NULL,
  hub                  REAL NOT NULL,
  ratio                REAL NOT NULL,
  manufacturingFactor  REAL NOT NULL,
  researchFactor       REAL NOT NULL
) STRICT;

-- Junction: which services (real, 2 to 24 per operation) a station with
-- this operation type offers.
CREATE TABLE stationOperationServices (
  operationId  INTEGER NOT NULL REFERENCES stationOperations(operationId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  serviceId    INTEGER NOT NULL REFERENCES stationServices(serviceId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (operationId, serviceId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

-- Junction: which station item type corresponds to this operation for
-- each station-size category. sizeKey is a bit-flag (1/2/4/8/16 in the
-- real data, confirmed against all 68 records) -- kept as a plain
-- INTEGER rather than decoded into named constants, since the SDE
-- itself doesn't document what each flag represents beyond the value.
-- Nullable at the parent level: present in 47/68 real records (69.1%).
CREATE TABLE stationOperationTypes (
  operationId  INTEGER NOT NULL REFERENCES stationOperations(operationId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  sizeKey      INTEGER NOT NULL,
  typeId       INTEGER NOT NULL REFERENCES invTypes(typeId)
                 ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (operationId, sizeKey) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;

CREATE TABLE npcStations (
  stationId                 INTEGER NOT NULL PRIMARY KEY,
  -- Nullable: missing in exactly 1 of 5210 real records (a special,
  -- singular station -- e.g. Thera-like -- whose orbitID also matches
  -- neither a real moon nor a real planet; see orbitMoonId/orbitPlanetId
  -- below).
  celestialIndex            INTEGER,
  operationId               INTEGER NOT NULL REFERENCES stationOperations(operationId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  -- The real SDE's `orbitID` can be either a moon or a planet
  -- (confirmed against real data: 76.5% moons, 23.5% planets, ~0.02%
  -- neither) -- split into two mutually-exclusive nullable columns
  -- instead of one plain INTEGER, since SQL has no way to declare a
  -- single foreign key conditional on two different target tables.
  -- Exactly one of the two is non-NULL for the vast majority of
  -- stations; both are NULL for the rare exception.
  orbitMoonId               INTEGER REFERENCES mapMoons(moonId)
                               ON UPDATE CASCADE ON DELETE SET NULL,
  orbitPlanetId             INTEGER REFERENCES mapPlanets(planetId)
                               ON UPDATE CASCADE ON DELETE SET NULL,
  -- Nullable: present in 3986/5210 real records (76.5%) -- exactly the
  -- stations that orbit a moon (a station orbiting a planet directly
  -- has no "index" to report, same absence rate as orbitMoonId being
  -- NULL).
  orbitIndex                INTEGER,
  ownerId                   INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  positionX REAL NOT NULL, positionY REAL NOT NULL, positionZ REAL NOT NULL,
  reprocessingEfficiency    REAL NOT NULL,
  reprocessingHangarFlag    INTEGER NOT NULL,
  reprocessingStationsTake  REAL NOT NULL,
  solarSystemId             INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  typeId                    INTEGER NOT NULL REFERENCES invTypes(typeId)
                               ON UPDATE CASCADE ON DELETE RESTRICT,
  useOperationName          INTEGER NOT NULL CHECK (useOperationName IN (0,1))
) STRICT;
CREATE INDEX idx_npcStations_solarSystemId ON npcStations(solarSystemId);
CREATE INDEX idx_npcStations_operationId ON npcStations(operationId);
CREATE INDEX idx_npcStations_ownerId ON npcStations(ownerId);
