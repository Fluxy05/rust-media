# RUST-MEDIA-PLAYER: Un Lecteur Vidéo Rust avec Décodage Matériel

Ce projet est un lecteur vidéo simple utilisant Rust avec le décodage matériel via VAAPI et NVDEC.

## Prérequis

- Rust et Cargo
- FFmpeg avec support VAAPI
- SDL2
- Pilotes VAAPI pour votre GPU
- Pilote NVIDIA officiel (open source driver can work but not officialy)
- Une distribution linux moderne

### Installation des dépendances sur Ubuntu/Debian

```bash
sudo apt update
sudo apt install libavcodec-dev libavformat-dev libavutil-dev libsdl2-dev vainfo libva-dev
```

### Installation des dépendances sur Arch Linux

```bash
sudo pacman -S ffmpeg sdl2 intel-media-driver (intel) mesa (amd) nvidia-utils (nvidia officiel)
```

## Compilation

```bash
cargo build --release
```

## Utilisation

Exécutez le programme en spécifiant le chemin de la vidéo comme argument :

```bash
cargo run --release -- /chemin/vers/votre/video.mp4
```

Ou après compilation :

```bash
./target/release/rust-media-player /chemin/vers/votre/video.mp4
```

## Contrôles

- ESC : Quitter le lecteur
- Fermer la fenêtre pour quitter

## Notes

- Le programme utilise VAAPI pour le décodage matériel, assurez-vous que votre GPU le supporte
- Vous pouvez vérifier le support VAAPI avec la commande `vainfo` 
