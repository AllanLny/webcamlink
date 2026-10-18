# Sécurité

## Signaler une vulnérabilité

Merci de **ne pas** ouvrir d'issue publique pour une vulnérabilité. Utilise de préférence
[GitHub Security Advisories](https://github.com/AllanLny/webcamlink/security/advisories/new)
(onglet *Security* → *Report a vulnerability*), ou écris à **allan1411lannoy@gmail.com**.

Décris le problème, la version concernée et, si possible, une reproduction. Je réponds sous quelques
jours et je te crédite dans le correctif si tu le souhaites.

## Périmètre

Le projet exécute du code à plusieurs niveaux, ce qui rend certains points sensibles :

- **Caméra virtuelle** : DLL chargée par le **Frame Server** (session 0, compte LOCAL SERVICE). Les
  objets de mémoire partagée `Global\…` sont créés par la DLL et donc lisibles par toute session : le
  format de l'en-tête et des slots est documenté dans [docs/shm.md](docs/shm.md) et doit être validé
  avant usage (taille, dimensions, index).
- **Agent PC** : session utilisateur, parle à l'iPhone via usbmuxd ; il ne doit jamais exécuter ce que
  l'iPhone envoie (le protocole WCLN est un protocole de données, pas de commandes).
- **Protocole WCLN** : toute longueur ou tout index reçu est analysé côté Rust et côté Swift ; une
  entrée invalide doit être rejetée, pas traitée.
- **Enregistrement COM** : écrit dans `HKLM` (catégorie `KSCATEGORY_VIDEO_CAMERA`) et doit référencer la
  DLL réellement installée — un testeur peut le vérifier avec `wcl-vcam-register.exe` (admin).
- **App iOS** : demande l'accès caméra et le réseau local ; le trafic reste local (USB ou réseau local).

## Versions prises en charge

Le projet est en développement actif : seules la branche `main` et la dernière release sont prises en
charge. Si tu découvres un problème dans une version ancienne, reproduis-le d'abord sur `main`.
