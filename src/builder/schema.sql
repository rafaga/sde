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

CREATE TABLE staStation (
  stationId INTEGER NOT NULL PRIMARY KEY,
  stationName TEXT NOT NULL,
  solarSystemId INTEGER REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE SET NULL,
  stationType INTEGER NOT NULL
) STRICT;
CREATE INDEX idx_staStation_solarSystemId ON staStation(solarSystemId);

CREATE TABLE staCorporations (
  solarSystemId INTEGER NOT NULL REFERENCES mapSolarSystems(solarSystemId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  corporationId INTEGER NOT NULL REFERENCES npcCorporations(corporationId)
                  ON UPDATE CASCADE ON DELETE CASCADE,
  CONSTRAINT pkey PRIMARY KEY (solarSystemId, corporationId) ON CONFLICT FAIL
) STRICT, WITHOUT ROWID;
